# Apple Music wrapper patch

This directory stores local patches for `WorldObservationLog/wrapper`.

`kd-context-cache.patch` caches FairPlay `kdContext` by `(adam, uri)` instead of
recreating a decrypt context for every fragment connection. With cached decrypt
input for track `1624001324`:

- single `fragment_parallel2`: `2.36s` decrypt, previously about `7.6s`
- two concurrent test processes: both succeeded, about `3.0s` decrypt each
- four concurrent test processes: all succeeded, about `4.46s` to `5.94s`

Build locally:

```sh
git clone --depth=1 https://github.com/WorldObservationLog/wrapper.git /tmp/apple-wrapper-src
git -C /tmp/apple-wrapper-src apply /path/to/bot-rs/docker/wrapper/kd-context-cache.patch
docker build -t musicbot-wrapper:kd-cache-test /tmp/apple-wrapper-src
```

Run with existing login data:

```sh
docker run -d --name applemusic-wrapper --privileged \
  -v /path/to/bot-rs/data/applemusic-wrapper:/app/rootfs/data \
  -p 10020:10020 -p 20020:20020 -p 30020:30020 \
  -e args="-H 0.0.0.0" \
  musicbot-wrapper:kd-cache-test
```
