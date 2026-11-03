# Third Party Notices

## MusicBot-Go

Music provider behavior and platform integration logic in `src/funcs/command/music*` is ported from or derived from:

- Project: MusicBot-Go
- Repository: https://github.com/liuran001/MusicBot-Go
- License: GNU General Public License v3.0

The built-in public Widevine L3 test device was restored from
[`plugins/applemusic/wv_device.go`](https://github.com/liuran001/MusicBot-Go/blob/0d4fe5d4e00e2709ab6fc692a1f09621f8e4b5af/plugins/applemusic/wv_device.go)
at commit `0d4fe5d4e00e2709ab6fc692a1f09621f8e4b5af`, serialized as WVD v2
(decoded SHA256: `3c0966faf5107be20e9584543bc7e7537f79ed96603bcab4176bae9a3542117a`).
It is upstream's intentionally shared test device, not a user's Apple account data.

When distributing this bot with the ported music provider code, keep this notice and comply with GPL-3.0 requirements for the combined work.

## Temari

The wrapper-lite transport in `src/funcs/command/music/applemusic/wrapper_lite.rs`
uses Temari 0.3.1 for local sample decryption, following the complete-file path
in the user's Music API project.

- Repository: https://github.com/WorldObservationLog/Temari
- License: MIT

The website-specific packet transport and WASM player are not included in this bot.

```text
MIT License

Copyright (c) 2025 Temari contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
