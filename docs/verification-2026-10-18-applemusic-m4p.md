# Apple Music protected M4P fallback verification — 2026-10-18

## Scope and cause

This change belongs to the music download bot. The website repository and its
deployed executable and entry page were not changed.

The failing catalog song was `1328749645`, “Kokoro Float (feat. nicamoq)” by
Yunomi. The configured storefront returned `lossy-stereo` and no `enhancedHls`.
The wrapper returned a protected `.m4p` file URL, not an HLS manifest. Both
existing wrapper transports expected HLS, so neither could handle that result.
A control song returned a valid HLS URL, indicating this was not a general
wrapper/session outage.

Live testing also exposed invalid Base64 in the embedded default Widevine
device. Its public upstream data was restored; provenance and the decoded
device digest are recorded in `THIRD_PARTY_NOTICES.md`. No user session values
or signed media URLs are included in this report.

## Behavior

- Standard/High can fall back from the configured wrapper transports to
  full-length supported WebPlayback AAC assets using the configured account.
- Lossless/Hi-Res never fall back to AAC. Failure explains that no usable stream
  at the requested lossless quality was obtained.
- Manifest URL validation distinguishes protected M4P responses from HLS.
  Error summaries preserve failure stages while removing signed media URLs.
- Asset metadata, manifest duration, and completed output validation guard
  against accidentally delivering a preview.

## Validation

- `cargo test --locked --lib --quiet`: **178 passed, 10 ignored**.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`: passed.
- `cargo +nightly fmt -- --check` and `git diff --check`: passed.
- Offline tests cover URL validation, error redaction, quality selection,
  preview rejection, lossless downgrade prevention, and a signed license
  challenge generated with the embedded device.
- A separate read-only review found no blocking issues.
- The isolated live test
  `live_high_specific_track_id_is_complete_without_telegram`, with
  `APPLE_MUSIC_TEST_ID=1328749645`, passed on the server in **5.97 seconds**.
  The resulting M4A contained **AAC**, measured **278.430476 seconds** and
  **9,575,440 bytes**, and passed a complete FFmpeg decode. Catalog duration
  was 278.361 seconds.

No Telegram messages or test audio uploads were sent. This validates download
and file decoding; it does not claim playback verification in a Telegram client.

## Deployment

Built the Linux executable in the existing `bot-rs-builder-amd64` container:

```sh
cargo build --locked --release --bin bot-rs \
  --target x86_64-unknown-linux-gnu --target-dir /cargo-cache/target -j 8 \
  --config profile.release.lto=false
```

The builder and cache were retained. Only the bot executable was replaced,
after backing up the previous executable and configuration. Only `bot.service`
was restarted; wrapper services and reverse proxy configuration were preserved.

- Release SHA-256:
  `109180aff51e910cfd7f1fd45c911afb5a0a8329644222f72474941b54e25a41`
- Previous executable SHA-256:
  `41357b7dba0a1269563a91409a5c75ebe3f49edc5ec1731ca976664eed449670`
- Backup directory:
  `/root/bot-rs/backups/m4p-fallback-20261018-6yas0s31`
- The running executable digest matches the release.
- Configuration digest is unchanged, and `MODE=r` is preserved.
- Service is active/running, restart count is zero, and the expected loopback
  listener is owned by the new process.
- Telegram `getMe` and `getWebhookInfo` succeeded. The registered webhook
  matches the configured URL after normalizing an empty root path to `/`.
  Pending updates were zero; no last webhook error was reported.
- No error-priority journal entries appeared after restart.
- Website executable and entry page digests are unchanged.

The song's observed catalog availability was lossy only. Users requesting this
song can select High; Lossless/Hi-Res intentionally continue to report that a
usable stream at that quality was not obtained.
