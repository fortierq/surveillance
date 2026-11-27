# Phone API

The host listens on loopback. A private HTTPS reverse proxy provides access from the paired phone. All paths are under `/v1`. Every request except pairing sends the device credential in an `Authorization: Bearer …` header. JSON responses use `Cache-Control: no-store`.

| Method and path | Contract |
| --- | --- |
| `POST /v1/pair` | Send `{"code":"…"}` to exchange a five-minute, one-time code for `{"credential":"…"}`. |
| `DELETE /v1/pair` | Revoke the authenticated phone and remove its pending alerts. |
| `GET /v1/status` | Return `armed`, `camera`, `microphone`, `push_configured`, and `push_registered`. |
| `PUT /v1/armed` | Send `{"armed":true}` or false. A successful update returns 204. |
| `GET /v1/events` | Return `events` in descending insertion order and a nullable `next_before` cursor. Pass the cursor as `before` to fetch older events. `limit` defaults to 50 and can be set up to 100. |
| `GET /v1/events/{id}` | Return the event with that UUID, or 404 once it has expired. |
| `PUT /v1/push-token` | Send `{"token":"…"}` when the Android FCM token is available or changes. |

An event contains `id`, `kind` (`motion`, `sound`, or `test`), `occurred_at_ms` (Unix epoch milliseconds), and `score` (changed image fraction or decibels above the noise floor). Sensor states are `disabled`, `idle`, `starting`, `ready`, `unavailable`, and on macOS `permission_needed` or `permission_denied`.
