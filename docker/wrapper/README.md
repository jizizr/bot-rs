# Apple Music wrapper patch

This directory stores local patches for `WorldObservationLog/wrapper`.

`kd-context-cache.patch` changes two things:

- caches FairPlay `kdContext` by `(adam, uri)` instead of recreating a decrypt
  context for every fragment connection
- handles decrypt-port connections (`10020`) in detached pthread workers, while
  protecting `kdContext` cache and lease refresh with mutexes

With cached decrypt input for track `1624001324`:

- single `fragment_parallel2`: `2.36s` decrypt, previously about `7.6s`
- two concurrent test processes: both succeeded, about `3.0s` decrypt each
- four concurrent test processes: all succeeded, about `4.46s` to `5.94s`

With the threaded decrypt listener, two different live lossless downloads were
also tested concurrently:

- `535824738` (`晴天`): succeeded in about `15.6s`
- `536115195` (`七里香`): succeeded in about `14.5s`

For real multi-track parallelism, run multiple wrapper processes and configure
the bot to use a wrapper pool. The FairPlay session inside one wrapper process
is not stable when several tracks decrypt through it at once; spreading tracks
across processes avoids that shared native state.

Example bot config:

```toml
[music.applemusic]
wrapper_hosts = [
  "127.0.0.1:10020",
  "127.0.0.1:10022",
  "127.0.0.1:10023",
  "127.0.0.1:10024",
]
wrapper_track_concurrency = 1
wrapper_fragment_concurrency = 1
wrapper_connection_concurrency = 4
```

The bot maps `127.0.0.1:10024` to decrypt port `10024` and m3u8 port `20024`.
When more than one endpoint is configured, Apple Music internal URLs use
`applemusic-wrapper://pool/...` and each track is round-robin assigned to one
wrapper process.

Each wrapper process must mount an independent copy of `/app/rootfs/data`.
Sharing the same data directory across wrapper processes can still reset the
native FairPlay session even when the bot queues one track per process.

Cached 20-track lossless decrypt benchmark on local Docker:

- one wrapper process, `track=2`: `40.73s`, 0 failures
- one wrapper process, `track=3` or `track=4`: wrapper reset/broken pipe
- four wrapper processes sharing one data directory: reset/timeouts
- four wrapper processes with independent data directories, `track=1` and
  `fragment=1`: `39.72s`, 0 failures

Build locally:

```sh
git clone --depth=1 https://github.com/WorldObservationLog/wrapper.git /tmp/apple-wrapper-src
git -C /tmp/apple-wrapper-src apply /path/to/bot-rs/docker/wrapper/kd-context-cache.patch
docker build -t musicbot-wrapper:threaded-test /tmp/apple-wrapper-src
```

Run with existing login data:

```sh
docker run -d --name applemusic-wrapper --privileged \
  -v /path/to/bot-rs/data/applemusic-wrapper:/app/rootfs/data \
  -p 10020:10020 -p 20020:20020 -p 30020:30020 \
  -e args="-H 0.0.0.0" \
  musicbot-wrapper:threaded-test
```
