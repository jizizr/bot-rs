# bot-rs
A simple framework has been completed

Music commands:
- `/music`: open the personal default-platform button panel.
- `/music <song>` or `/music get <song>`: download using the saved default platform.
- `/music search <song>`: search using the saved default platform.
- `/music setting`: configure the default platform, quality, cover, and lyric script.
- `/music collection <link-or-id>`: open a collection using the saved default platform.

Platform buttons save the user's preference. Command-level platform overrides are not supported.

Apple Music transport, Telegram audio compatibility, and standalone verification:
[docs/applemusic.md](docs/applemusic.md).
