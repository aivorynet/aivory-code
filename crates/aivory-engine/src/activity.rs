//! The pulse chart: one sample per minute, the last hour.
//!
//! [`Sampler`] keeps a ring of at most [`ACTIVITY_SAMPLES`] samples. The daemon calls
//! [`Sampler::record_and_save`] whenever a session changes state and at least once per minute.
//! After a restart, [`Sampler::load`] restores the ring from the store.
//!
//! What a sample counts:
//!
//! - Every live agent counts once: the main agent of a live session by the state of the session,
//!   and every subagent by its own state.
//! - `active` counts working agents. `waiting` counts agents that need the user, errors included.
//!   `idle` counts idle agents.
//! - A parked or done session counts nothing. A finished subagent counts nothing either. This
//!   differs from `WarRoom::counts().agents` in one case: that count also includes the finished
//!   subagents of a live session.
//!
//! Several records inside one minute merge into one sample. Each field keeps the peak of the
//! minute, so a short burst of work still shows in the chart.

use std::collections::VecDeque;

use aivory_protocol::{ACTIVITY_SAMPLES, ActivitySample, MINUTE_MS, Pane, SessionState, UnixMs};

use crate::store::{Result, Store};

/// The length of the chart in milliseconds. A sample older than this is dropped.
pub const WINDOW_MS: i64 = ACTIVITY_SAMPLES as i64 * MINUTE_MS;

/// The ring of the last [`ACTIVITY_SAMPLES`] minutes, oldest first. A minute without a record has
/// no sample.
#[derive(Debug, Clone, Default)]
pub struct Sampler {
    ring: VecDeque<ActivitySample>,
}

impl Sampler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Restore the ring after a restart. Samples that are 60 minutes old or older at `now` are
    /// dropped (a sample at `at` is kept while `now - at < 60 min`).
    pub fn load(store: &Store, now: UnixMs) -> Result<Self> {
        let rows = store.activity_since(now - WINDOW_MS)?;
        let skip = rows.len().saturating_sub(ACTIVITY_SAMPLES);
        Ok(Self {
            ring: rows.into_iter().skip(skip).collect(),
        })
    }

    /// Count the agents of `panes` and merge the counts into the sample of the minute of `now`.
    /// Returns that sample. A clock that steps back merges into the newest sample.
    pub fn record(&mut self, now: UnixMs, panes: &[Pane]) -> ActivitySample {
        let (active, waiting, idle) = count_agents(panes);
        let fresh = ActivitySample {
            at: now - now.rem_euclid(MINUTE_MS),
            active,
            waiting,
            idle,
        };
        let sample = match self.ring.back_mut() {
            Some(last) if fresh.at <= last.at => {
                last.active = last.active.max(active);
                last.waiting = last.waiting.max(waiting);
                last.idle = last.idle.max(idle);
                *last
            }
            _ => {
                self.ring.push_back(fresh);
                fresh
            }
        };
        while self
            .ring
            .front()
            .is_some_and(|f| now - f.at >= WINDOW_MS || self.ring.len() > ACTIVITY_SAMPLES)
        {
            self.ring.pop_front();
        }
        sample
    }

    /// [`Sampler::record`], then save the merged sample and delete the stored samples that fell
    /// out of the window.
    pub fn record_and_save(
        &mut self,
        store: &mut Store,
        now: UnixMs,
        panes: &[Pane],
    ) -> Result<ActivitySample> {
        let sample = self.record(now, panes);
        store.put_activity(&sample)?;
        store.prune_activity(now - WINDOW_MS)?;
        Ok(sample)
    }

    /// The samples, oldest first. This is `WarRoom::activity`.
    pub fn samples(&self) -> Vec<ActivitySample> {
        self.ring.iter().copied().collect()
    }

    pub fn len(&self) -> usize {
        self.ring.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ring.is_empty()
    }
}

/// `(active, waiting, idle)` agents, see the module docs.
fn count_agents(panes: &[Pane]) -> (u32, u32, u32) {
    let mut counts = (0, 0, 0);
    let mut add = |state: SessionState| match state {
        SessionState::Working => counts.0 += 1,
        SessionState::NeedsYou | SessionState::Error => counts.1 += 1,
        SessionState::Idle => counts.2 += 1,
        SessionState::Parked | SessionState::Done => {}
    };
    for pane in panes.iter().filter(|p| p.state.is_live()) {
        add(pane.state);
        for sub in &pane.subagents {
            add(sub.state);
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{pane, sub};

    const T0: UnixMs = 1_800_000_000_000 - 1_800_000_000_000 % MINUTE_MS;

    fn mixed() -> Vec<Pane> {
        let mut main = pane("a", "web", SessionState::Working, T0);
        main.subagents = vec![
            sub("s1", SessionState::Working),
            sub("s2", SessionState::Idle),
            sub("s3", SessionState::Done),
        ];
        vec![
            main,
            pane("b", "web", SessionState::NeedsYou, T0),
            pane("c", "api", SessionState::Error, T0),
            pane("d", "api", SessionState::Idle, T0),
            pane("e", "api", SessionState::Parked, T0),
            pane("f", "api", SessionState::Done, T0),
        ]
    }

    #[test]
    fn counts_active_waiting_idle_with_subagents() {
        let mut s = Sampler::new();
        let sample = s.record(T0 + 5_000, &mixed());
        assert_eq!(
            sample,
            ActivitySample {
                at: T0,
                active: 2,  // a + s1
                waiting: 2, // b (needs you) + c (error)
                idle: 2,    // d + s2
            }
        );
    }

    #[test]
    fn parked_session_counts_nothing_even_with_subagents() {
        let mut parked = pane("p", "web", SessionState::Parked, T0);
        parked.subagents = vec![sub("s", SessionState::Working)];
        let sample = Sampler::new().record(T0, &[parked]);
        assert_eq!((sample.active, sample.waiting, sample.idle), (0, 0, 0));
    }

    #[test]
    fn records_in_one_minute_merge_to_the_peak() {
        let mut s = Sampler::new();
        let one = |state| vec![pane("a", "web", state, T0)];
        s.record(T0 + 1_000, &one(SessionState::Working));
        s.record(T0 + 20_000, &one(SessionState::NeedsYou));
        let merged = s.record(T0 + 59_999, &one(SessionState::Idle));
        assert_eq!(s.len(), 1);
        assert_eq!(
            merged,
            ActivitySample {
                at: T0,
                active: 1,
                waiting: 1,
                idle: 1
            }
        );
        assert_eq!(s.samples(), vec![merged]);
    }

    #[test]
    fn next_minute_appends_a_sample() {
        let mut s = Sampler::new();
        s.record(T0, &mixed());
        let next = s.record(T0 + MINUTE_MS, &[]);
        assert_eq!(next.at, T0 + MINUTE_MS);
        assert_eq!((next.active, next.waiting, next.idle), (0, 0, 0));
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn ring_keeps_60_samples() {
        let mut s = Sampler::new();
        for m in 0..75 {
            s.record(T0 + m * MINUTE_MS, &mixed());
        }
        let samples = s.samples();
        assert_eq!(samples.len(), ACTIVITY_SAMPLES);
        assert_eq!(samples[0].at, T0 + 15 * MINUTE_MS);
        assert_eq!(samples[59].at, T0 + 74 * MINUTE_MS);
    }

    #[test]
    fn gap_drops_samples_older_than_60_minutes() {
        let mut s = Sampler::new();
        s.record(T0, &mixed());
        s.record(T0 + 30 * MINUTE_MS, &mixed());
        // 61 min after the first sample: only the second one stays.
        s.record(T0 + 61 * MINUTE_MS, &mixed());
        let ats: Vec<_> = s.samples().iter().map(|x| x.at).collect();
        assert_eq!(ats, vec![T0 + 30 * MINUTE_MS, T0 + 61 * MINUTE_MS]);
    }

    #[test]
    fn clock_step_back_merges_into_newest() {
        let mut s = Sampler::new();
        s.record(T0 + 5 * MINUTE_MS, &[]);
        let sample = s.record(T0, &mixed());
        assert_eq!(sample.at, T0 + 5 * MINUTE_MS);
        assert_eq!(sample.active, 2);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn restore_after_restart_and_keep_merging() {
        let mut store = Store::open_in_memory().unwrap();
        let mut s = Sampler::new();
        for m in 0..3 {
            s.record_and_save(&mut store, T0 + m * MINUTE_MS, &mixed())
                .unwrap();
        }
        let now = T0 + 2 * MINUTE_MS + 10_000;
        let mut restored = Sampler::load(&store, now).unwrap();
        assert_eq!(restored.samples(), s.samples());
        // The same minute merges into the restored sample; the peak stays.
        let again = restored
            .record_and_save(&mut store, now, &[pane("x", "web", SessionState::Idle, T0)])
            .unwrap();
        assert_eq!(again.at, T0 + 2 * MINUTE_MS);
        assert_eq!(again.active, 2);
        assert_eq!(restored.len(), 3);
        assert_eq!(store.activity_since(0).unwrap().len(), 3);
    }

    #[test]
    fn load_drops_samples_older_than_60_minutes() {
        let mut store = Store::open_in_memory().unwrap();
        for m in [0, 1, 2, 59, 60] {
            let at = T0 + m * MINUTE_MS;
            store
                .put_activity(&ActivitySample {
                    at,
                    active: 1,
                    waiting: 0,
                    idle: 0,
                })
                .unwrap();
        }
        // 60 minutes after sample 0: sample 0 is exactly 60 min old and drops.
        let now = T0 + 60 * MINUTE_MS;
        let s = Sampler::load(&store, now).unwrap();
        let ats: Vec<_> = s
            .samples()
            .iter()
            .map(|x| (x.at - T0) / MINUTE_MS)
            .collect();
        assert_eq!(ats, vec![1, 2, 59, 60]);
        // One minute later, sample 1 drops too.
        let s = Sampler::load(&store, now + MINUTE_MS).unwrap();
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn record_and_save_prunes_old_rows() {
        let mut store = Store::open_in_memory().unwrap();
        let mut s = Sampler::new();
        s.record_and_save(&mut store, T0, &mixed()).unwrap();
        s.record_and_save(&mut store, T0 + 90 * MINUTE_MS, &mixed())
            .unwrap();
        let rows = store.activity_since(0).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].at, T0 + 90 * MINUTE_MS);
    }
}
