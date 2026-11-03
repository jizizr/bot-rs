# Apple Music: wrapper-lite and Telegram output

The provider can use the wrapper-lite infrastructure shared with the Music API
project. Only the complete-file path is used here; the website's SPLALAC1/WASM
packet transport is not a Telegram audio format.

## Configuration

Merge these fields into the existing sections in `data/conf.toml`:

```toml
[music.applemusic]
wrapper_lite_enabled = true
wrapper_lite_url = "http://127.0.0.1:12340"
wrapper_lite_urls = []
wrapper_lite_batch_size = 256
# Optional legacy fallback. Keep its existing safe concurrency settings.
wrapper_host = "127.0.0.1"

[music.telegram_audio]
ffmpeg = "ffmpeg"
ffprobe = "ffprobe"
timeout_seconds = 180
concurrency = 2
```

wrapper-lite is disabled by default. When enabled, at least one valid URL must
be supplied. `wrapper_lite_urls` can list additional endpoints. Keep this service
on loopback or a private network: its key endpoint must not be exposed publicly.
The existing wrapper login is maintained by that service, not by the bot.

Install FFmpeg and FFprobe on the bot host (or set their absolute paths above).
They are required for Apple Music uploads even when using the legacy transport.
No Telegram token or database is needed to run the isolated media tests below.

## Download Path

1. Resolve the catalog song and expected duration. Bare numeric input first
   attempts a song ID lookup; only a 404 falls back to title search. An explicitly
   selected ID, `am:ID`, or song URL never falls back to a different song.
2. Request the master playlist from wrapper-lite `/m3u8`; select the user's
   requested quality. High is still AAC source, not an alias for lossless.
3. Reject preview-length playlists and download the complete encrypted MP4.
4. Request `/key` templates and decrypt samples locally with Temari. Templates
   are kept in memory for at most 15 minutes (up to 64 cached entries). Unsupported
   CENC layouts and wrapper-lite failures fall back to the existing TCP wrapper
   when configured. If both transports fail, Standard/High may use the configured
   account's WebPlayback Widevine AAC assets; this also handles catalog songs
   whose wrapper response is a protected `.m4p` file instead of an HLS playlist.
   Only supported CTR AAC assets are selected, using the requested quality target
   and excluding known preview-length assets. This fallback never converts a
   Lossless/Hi-Res request to AAC. Signed media URLs are omitted from errors.
5. Rebuild progressive MP4 and apply the existing ALAC repair.
6. Remux into standard M4A without re-encoding and validate the complete output
   before upload.

The existing wrapper admission semaphore still limits simultaneous tracks.
Local sample batches are parallelized by Temari, without concurrent writes into
the legacy wrapper session.

## Playback Contract

The selected source quality is preserved: high/standard stays AAC and
lossless/Hi-Res stays ALAC, both in a progressive M4A container. FFmpeg uses
`-c:a copy`, with no resampling, channel mixing or lossy transcoding. Codec,
profile, sample rate, channel count and reported bit depth must match before
and after remuxing. Unknown codecs are rejected instead of silently downgraded.
M4A is a container, not a codec; an ALAC M4A remains lossless. This does not
claim that every Telegram client has been tested for ALAC playback.

FFmpeg checks decoding, FFprobe checks codec/duration, and the completed output
is decoded end to end before sending. Failed checks prevent upload and cache
insertion. Temporary files are removed when processing finishes or fails.

Apple Music Telegram cache keys use a `:tg-m4a-v2` quality suffix. Existing
unvalidated cache entries and the earlier experimental `:tg-aac-v1` lossy
derivatives are not reused or deleted. Captions use the validated file's actual
duration, bitrate and file size.

## Standalone Verification

Normal unit tests (no network or FFmpeg required):

```sh
cargo test --locked --lib
```

Offline AAC/ALAC packet preservation (including 24-bit / 96 kHz ALAC), and
invalid/preview rejection:

```sh
cargo test --locked --lib preserves_alac_and_aac_packets_without_transcoding -- --ignored --nocapture
```

Live catalog regression, without downloading or contacting Telegram:

```sh
cargo test --locked --lib live_numeric_title_resolves_without_telegram -- --ignored --nocapture
```

Full wrapper-lite high/lossless downloads, followed by Telegram compatibility
preparation and complete decoding (requires an authenticated wrapper-lite):

```sh
APPLE_MUSIC_TEST_WRAPPER_LITE_URL=http://127.0.0.1:12340 \
APPLE_MUSIC_TEST_ID=1624001324 \
cargo test --locked --release --config profile.release.lto=false --lib \
  live_wrapper_lite_high_and_lossless_are_playable -- --ignored --nocapture
```

Do not run all ignored tests indiscriminately: other integration tests may use
Telegram or live platform credentials. A successful offline/full-file decode is
not a replacement for final client playback verification after deployment.

## Protected M4P responses

Some songs expose only lossy-stereo catalog assets and omit `enhancedHls`.
The wrapper's `/m3u8` result can then be a signed `.m4p` file address. It is not
an HLS manifest and must not be parsed as one. Standard/High can try WebPlayback
for a full-length supported AAC stream; Lossless/Hi-Res reports that no usable
lossless stream was obtained and keeps the requested quality unchanged. A
successful search alone does not establish lossless availability.

The isolated High test exercises the complete transport fallback without
contacting Telegram; it checks the resulting AAC file with FFprobe and decodes
it fully with FFmpeg:

```sh
APPLE_MUSIC_TEST_ID=1328749645 \
cargo test --locked --release --config profile.release.lto=false --lib \
  live_high_specific_track_id_is_complete_without_telegram -- --ignored --nocapture
```
