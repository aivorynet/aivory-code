# ADR 0015 — Personal remote access with iroh and push

Status: Accepted, 2026-10-21

## Context
The phone must reach the user's daemon from any network, approve requests, and show live
sessions. iOS stops background connections. Claude Remote Control is first-party only.

## Decision
- iroh (QUIC with hole punching) between the phone and the daemon. Our own iroh relay on the
  Hetzner K3s cluster carries the traffic only when no direct path exists.
- Pairing: the desktop shows a QR code with the daemon key and a one-time secret. The device
  key goes into the device registry in aivory-guard (P5).
- Push: APNs and FCM carry an encrypted payload (session id, event kind, short text). The
  phone decrypts it with the device key.

## Rejected alternatives
- **Tailscale:** needs a separate account and app; not under our control.
- **WebSocket relay through our servers for all traffic:** higher cost and latency; the relay
  sees metadata of all traffic.

## Consequences
- The relay and push are hosted services. They are part of the one-time Personal license (ADR 0017).
- Users can run their own relay (the iroh relay is open source) and skip our relay.

## Amended 2026-10-21 (first version)
- The first version pairs a phone with a code and a token on the local network (ADR 0021).
  The daemon keeps only the SHA-256 of the token in a local `devices.json`. The device key and
  the device registry in aivory-guard come with P5.
- This ADR still holds for the relay and for access outside the local network (P5).
  ADR 0021 uses no server of ours and replaces nothing here.
