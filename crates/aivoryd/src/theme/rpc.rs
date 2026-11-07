//! The `theme/*` and `registry/*` methods. A download runs on a worker thread; the method
//! returns a job, and the client polls `theme/job`.

use aivory_protocol::rpc::{Ack, Empty, code};
use aivory_protocol::theme::{
    RegistryAddParams, RegistryRef, RegistryRefreshParams, RemoveResult, ThemeAddParams,
    ThemeAssetParams, ThemeGetParams, ThemeRef, ThemeSourceParams,
};
use serde_json::{Value, json};

use super::store::valid_id;
use crate::outbox::Outbox;
use crate::rpc::{Daemon, RpcError, parse_params};

const METHODS: &[&str] = &[
    "theme/inspect",
    "theme/add",
    "theme/list",
    "theme/get",
    "theme/asset",
    "theme/remove",
    "theme/checkUpdates",
    "theme/update",
    "theme/diff",
    "theme/job",
    "registry/list",
    "registry/add",
    "registry/remove",
    "registry/refresh",
];

/// True for a method of this module.
pub fn handles(method: &str) -> bool {
    METHODS.contains(&method)
}

fn value<T: serde::Serialize>(v: &T) -> Result<Option<Value>, RpcError> {
    serde_json::to_value(v)
        .map(Some)
        .map_err(|e| RpcError::new(code::SERVER_ERROR, e.to_string()))
}

fn state(message: String) -> RpcError {
    RpcError::new(code::INVALID_STATE, message)
}

fn params_error(message: String) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, message)
}

fn theme_id(id: &str) -> Result<(), RpcError> {
    if valid_id(id) {
        Ok(())
    } else {
        Err(params_error(format!("\"{id}\" is not a theme id")))
    }
}

/// Run one method. `handles(method)` must be true.
pub fn dispatch(
    d: &Daemon,
    method: &str,
    params: Value,
    id: &Value,
    out: &Outbox,
) -> Result<Option<Value>, RpcError> {
    let themes = d.themes();
    match method {
        "theme/inspect" => {
            let p: ThemeSourceParams = parse_params(params)?;
            value(&themes.job_inspect(p.source).map_err(state)?)
        }
        "theme/add" => {
            let p: ThemeAddParams = parse_params(params)?;
            value(&themes.add(&p.token).map_err(|e| {
                if e.contains("token") {
                    params_error(e)
                } else {
                    state(e)
                }
            })?)
        }
        "theme/list" => {
            let _: Empty = parse_params(params)?;
            value(&themes.list())
        }
        "theme/get" => {
            let p: ThemeGetParams = parse_params(params)?;
            theme_id(&p.id)?;
            if !themes.store().exists(&p.id) {
                return Err(RpcError::not_found("theme", &p.id));
            }
            value(&super::bundle::get(themes.store(), &p.id, p.display).map_err(state)?)
        }
        "theme/asset" => {
            let p: ThemeAssetParams = parse_params(params)?;
            theme_id(&p.id)?;
            if !themes.store().exists(&p.id) {
                return Err(RpcError::not_found("theme", &p.id));
            }
            value(&super::bundle::asset(themes.store(), &p.id, &p.path).map_err(params_error)?)
        }
        "theme/remove" => {
            let p: ThemeRef = parse_params(params)?;
            theme_id(&p.id)?;
            if !themes.store().exists(&p.id) {
                return Err(RpcError::not_found("theme", &p.id));
            }
            themes.remove(&p.id).map_err(state)?;
            let cleared = clear_current(d, &p.id, id, out)?;
            value(&RemoveResult {
                ok: true,
                cleared_current: cleared,
            })
        }
        "theme/checkUpdates" => {
            let _: Empty = parse_params(params)?;
            value(&themes.job_check_updates().map_err(state)?)
        }
        "theme/update" => {
            let p: ThemeRef = parse_params(params)?;
            theme_id(&p.id)?;
            value(&themes.job_update(p.id).map_err(state)?)
        }
        "theme/diff" => {
            let p: ThemeRef = parse_params(params)?;
            theme_id(&p.id)?;
            value(&themes.job_diff(p.id).map_err(state)?)
        }
        "theme/job" => {
            let p: aivory_protocol::theme::JobRef = parse_params(params)?;
            themes
                .job(&p.job)
                .map_or_else(|| Err(RpcError::not_found("job", &p.job)), |j| value(&j))
        }
        "registry/list" => {
            let _: Empty = parse_params(params)?;
            value(&themes.registry_list())
        }
        "registry/add" => {
            let p: RegistryAddParams = parse_params(params)?;
            value(&themes.registry_add(&p.name, &p.url).map_err(params_error)?)
        }
        "registry/remove" => {
            let p: RegistryRef = parse_params(params)?;
            themes.registry_remove(&p.name).map_err(params_error)?;
            value(&Ack::OK)
        }
        "registry/refresh" => {
            let p: RegistryRefreshParams = parse_params(params)?;
            value(&themes.job_registry_refresh(p.name).map_err(state)?)
        }
        other => Err(RpcError::new(
            code::METHOD_NOT_FOUND,
            format!("unknown method '{other}'"),
        )),
    }
}

/// Removing the current theme clears `display.customTheme`. Without a war-room engine the
/// daemon has no settings, and nothing changes.
fn clear_current(d: &Daemon, removed: &str, id: &Value, out: &Outbox) -> Result<bool, RpcError> {
    let Some(view) = d.war_call("settings/get", json!({}), id, out) else {
        return Ok(false);
    };
    let current = view?.and_then(|v| {
        v["settings"]["display"]["customTheme"]
            .as_str()
            .map(str::to_string)
    });
    if current.as_deref() != Some(removed) {
        return Ok(false);
    }
    if let Some(r) = d.war_call(
        "settings/set",
        json!({ "key": "display.customTheme", "value": null }),
        id,
        out,
    ) {
        r?;
    }
    Ok(true)
}
