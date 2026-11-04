# bot-rs
A simple framework has been completed, built on [telers](https://github.com/Desiders/telers).

Music commands:
- `/music`: open the personal default-platform button panel.
- `/music <song>` or `/music search <song>`: list results from the saved default platform, with pages and buttons to switch platform; tapping a number fetches that song. A link, `platform:id` or bare song ID is fetched straight away.
- `/music get <song>`: fetch the first result directly.
- Inline mode (`@bot <song> [platform] [quality]`): the button above the results opens a private-chat platform picker that returns to the same search on another platform. Inline lyrics (`@bot 歌词 <song>`) and inline settings are rich messages too.
- Search results, collections, the music panel and settings, favorites and the inline platform picker are rich messages (headings, numbered lists, footers); song files stay audio messages, since Telegram allows only ordinary formatting in captions.
- The lyrics button on a song sends a rich message (Bot API rich messages): the cover, the song with its singer, platform and album, each line with its time and its translation under it in a quote that opens on demand, folded credits, and buttons to switch simplified/traditional script or get the lyrics as an LRC file. Songs sent inline arrive as rich cards (cover, name, byline, player), since an audio message cannot be edited into rich content; their lyrics button turns the card into the same lyric page with the player inside it, and "收起歌词" folds it back. Every lyric page — chat, private chat, inline results, LRC file captions — is rendered from one view, so they all look alike; covers become a short 3:1 banner (the cover over a blurred copy of itself, since Telegram shows rich-message pictures at full width with no size control) uploaded once to the shared storage chat (`[music] inline_upload_chat_id`), so inline pages show them too.
- `/curl <url>` answers with a rich page: status, address and network in a table, the certificate and headers folded, and the body on Paste.
- `/music setting`: configure the default platform, quality, cover, and lyric script.
- `/music collection <link-or-id>`: open a collection using the saved default platform.

Platform buttons save the user's preference. Command-level platform overrides are not supported.

### Music sources

The bot keeps no platform accounts or provider code. Search, links, lyrics and
downloads go through the loopback gateway of [MHub](https://github.com/jizizr/mhub),
which holds every account, downloads each song once and uploads it to the
Telegram cache shared with its web player; the bot sends the resulting file by
`file_id`. Both use the same bot token. Configure only the gateway here:

```toml
[music.gateway]
url = "http://127.0.0.1:8091"          # MHub's [gateway] listen
token_file = "/etc/bot-rs/gateway.token" # the same secret as MHub's gateway
# media_timeout_seconds = 900
```

Per-user and per-chat limits, the download queue and favorites stay in the bot.
The contract is the `music-gateway` crate in MHub's repository: pin its `rev` in
`Cargo.toml` for releases; for local development, point
`.cargo/config.toml` (ignored by Git) at a checkout:

```toml
[patch."https://github.com/jizizr/mhub"]
music-gateway = { path = "../music/crates/music-gateway" }
```
