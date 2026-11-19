<!-- atmosphere · api.md · Guide to the local API other apps can use -->
# Atmosphere local API

Atmosphere can let other apps on the same computer see what's playing and control playback. Examples include stream overlays, status tools, keyboard macros and your own scripts.

It's **off by default**. Turn it on in **Settings → Connections**, then create an access token.

## Security model

- **Same computer only.** The server listens on `127.0.0.1` and can't be reached from your network.
- **Token required.** Every request must send `Authorization: Bearer <token>`.
  - The token is 256 bits of randomness, shown once.
  - Atmosphere stores only its SHA-256 fingerprint, and checks it in constant time.
  - Replacing the token disconnects every app that used the old one.
- **Permissions you choose.** Each area can be switched off separately:
  - *See what's playing and browse the library*
  - *Control playback and the queue*
  - *Send links to Get*
  
  Links only open in Atmosphere. Nothing is saved until you review and confirm it there.
- **No web pages.** Requests carrying browser headers (`Origin`, `Sec-Fetch-Site`) are refused, and so are requests whose `Host` isn't `127.0.0.1:<port>` or `localhost:<port>`. This blocks malicious sites and DNS-rebinding tricks.
- **Brute-force lockout.** After 20 wrong tokens within a minute, every request gets `429` until the minute passes.
- **Small bodies only.** Request bodies over 16 KB are rejected.

## Basics

- **Base URL:** `http://127.0.0.1:47823`. The port can be changed in Settings.
- **Data format:** requests and responses are JSON.
- **Errors:** errors look like `{ "error": "message" }`.

```bash
curl -H "Authorization: Bearer $ATMO_TOKEN" http://127.0.0.1:47823/v1/ping
```

| Status | Meaning |
| --- | --- |
| `200` / `202` | OK / accepted (the app carries the action out right away) |
| `400` | Invalid input |
| `401` | Missing or wrong token |
| `403` | That permission is off, or the request came from a web page |
| `404` | Unknown endpoint, track or artwork |
| `413` | Body too large |
| `421` | Wrong `Host` header |
| `429` | Locked out after too many wrong tokens |

## Endpoints

### `GET /v1/ping`
This works with any permission. It returns the app version and which permissions are on.

```json
{ "app": "atmosphere", "version": "0.1.0", "allowed": { "read": true, "control": true, "links": false } }
```

### `GET /v1/now-playing` · read
```json
{
  "playing": true,
  "track": { "id": "…", "title": "Night Drive", "artist": "Nova Lumen", "album": "Glass Horizon", "albumArtist": "Nova Lumen", "durationMs": 251000, "artwork": "/v1/artwork/…" },
  "positionMs": 73120,
  "updatedAt": 1763500000000,
  "volume": 0.85, "muted": false, "shuffle": false, "repeat": "off",
  "queue": { "index": 3, "length": 12 }
}
```
`positionMs` was measured at `updatedAt`, a Unix time in milliseconds. While `playing` is true, add the time elapsed since then to get the current position.

### `GET /v1/events` · read
This is a [Server-Sent Events](https://developer.mozilla.org/docs/Web/API/Server-sent_events) stream. Every message is a `now-playing` event with the same body as `/v1/now-playing`:
- one is sent right away
- another follows whenever the track, play state, queue, volume or position jumps
- one also comes every 10 s while playing

```bash
curl -N -H "Authorization: Bearer $ATMO_TOKEN" http://127.0.0.1:47823/v1/events
```

### `GET /v1/library/tracks?q=<text>&limit=<1-500>` · read
This searches titles, artists and albums. The default `limit` is 50.

### `GET /v1/library/playlists` · read
This lists each playlist's name, its `trackIds` in order, and whether it's a linked folder that's currently missing.

### `GET /v1/artwork/{trackId}` · read
This returns the cover art as JPEG.

### `POST /v1/player/{action}` · control
The `action` is one of `play`, `pause`, `toggle`, `next` or `previous`.

### `POST /v1/player/seek` · control
```json
{ "position": 42.5 }
```
The position is in seconds.

### `POST /v1/player/volume` · control
```json
{ "volume": 0.6 }
```
The volume is between 0 and 1.

### `POST /v1/queue` · control
```json
{ "ids": ["trackId", "…"], "mode": "next" }
```
`mode` is one of three values:
- `now` replaces the queue and plays
- `next` plays after the current song
- `end` adds to the end, which is the default

Unknown ids are skipped. Send between 1 and 500 ids.

### `POST /v1/links` · links
```json
{ "url": "https://soundcloud.com/artist/track" }
```
This opens the link in Atmosphere's Get view so you can review it. It never downloads anything by itself.

## Example: a now-playing overlay feed in Python

```python
import json, os, urllib.request

req = urllib.request.Request("http://127.0.0.1:47823/v1/events", headers={"Authorization": f"Bearer {os.environ['ATMO_TOKEN']}"})
with urllib.request.urlopen(req) as stream:
    for line in stream:
        if line.startswith(b"data:"):
            now = json.loads(line[5:])
            if now["track"]:
                print(("▶ " if now["playing"] else "⏸ ") + f"{now['track']['artist']} — {now['track']['title']}")
```
