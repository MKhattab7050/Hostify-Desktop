# Sample applications

Real small applications used to test Hostify end to end. They are test fixtures, not templates.

| Folder | Type | What it proves |
|---|---|---|
| `static-html` | Static | V1 file hosting still works |
| `express-http` | Node.js | Dependency install, `PORT`, environment variables, crash and restart |
| `socketio-chat` | Node.js | A WebSocket stays connected through Caddy and the tunnel |
| `node-mysql` | Node.js + MySQL | Database credentials from environment variables, prepared statements |
| `php-basic` | PHP | FastCGI, `$_SERVER`, environment variables |
| `php-mysql` | PHP + MySQL | PDO with prepared statements |

Every server application answers `GET /health` with `{"ok":true,...}` so a check can tell it apart from a generic success page.

`GET /crash` in `express-http` exits the process on purpose, to test restart behaviour. Do not publish that sample on a domain you care about.
