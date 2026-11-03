# Third Party Notices

## MusicBot-Go

Music provider behavior and platform integration logic in `src/funcs/command/music*` is ported from or derived from:

- Project: MusicBot-Go
- Repository: https://github.com/liuran001/MusicBot-Go
- License: GNU General Public License v3.0

Platform implementations, accounts and downloads now live in MHub
(https://github.com/jizizr/mhub, AGPL-3.0-only), which the bot reaches through
the `music-gateway` crate of that repository. That crate keeps MHub's license.

When distributing this bot with the ported music provider code, keep this notice and comply with GPL-3.0 requirements for the combined work.
