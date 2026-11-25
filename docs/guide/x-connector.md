# X connector

The X connector has two capabilities with different authentication requirements. Both use
the single credential stored on the connection; ORBIT treats that credential as a bearer
token and never returns it after creation.

| Capability | Effect | How it runs | Token |
|---|---|---|---|
| `posts.search` | `read` | Pre-execution hydration: recent posts are added to the run input as `externalContext` with `provenance.trusted=false`. It is not exposed as an agent tool. | App-only bearer token is sufficient |
| `posts.create` | `external_side_effect` | Tool broker HTTP tool `x_posts_create`, calling `POST {baseUrl}/tweets`. | **User-context** access token with `tweet.write` |

`posts.create` has the `external_side_effect` effect, so the normal autonomy, approval and
tool-policy rules apply: below L4 it needs approval unless a policy permits it, and it is
denied at low autonomy levels. The broker selects the X client only for connections whose
connector slug is `x`; it never routes X calls to the GitHub client.

## Create the connection

1. In X's developer portal create an app with **OAuth 2.0** user-context authentication
   (Authorization Code with PKCE) and the `tweet.write` scope, or use OAuth 1.0a user
   context. An app-only bearer token can search but cannot post.
2. Obtain a **user access token** for the account that will publish. Tokens are not
   refresh-rotated by ORBIT in this slice; obtain a new token when the old one expires.
3. In Connection Studio choose **X**, enter a name, paste the user access token into the
   **Credential** field, and tick only the capabilities the agent needs.
4. Optionally set `baseUrl` in the connection config (default `https://api.x.com/2`) and
   `query` for `posts.search` hydration. Hydration runs only with a query, from the
   connection config or `{"query": "..."}` in the run payload; the task text is never sent
   to X. Tool policy rules for `posts.search` apply to hydration: a deny or
   require-approval rule skips it, and each search shows up as a tool call in the
   execution's events.

Credentials are write-only. The token is encrypted at rest, leased to the worker for the
call, and never written to prompts, execution events, audit logs or workspace files. The
broker records the capability, effect, decision and argument/result summaries only.

## Publish or reply

`x_posts_create` accepts:

```json
{
  "text": "Hello from ORBIT",
  "replyTo": "1234567890123456789"
}
```

`text` is required and validated against X's limit (280 weighted characters: URLs count
as 23, CJK and emoji as 2). For a reply, use either `replyTo` or
`reply.in_reply_to_tweet_id` with a numeric tweet ID. Both forms produce the X API body:

```json
{
  "text": "Hello from ORBIT",
  "reply": { "in_reply_to_tweet_id": "1234567890123456789" }
}
```

If X returns 401/403 — including an app-only token — the tool error explains that a
user-context token with `tweet.write` is required. It does not include the token or X's
raw response body.

## Mock and live verification

Unit and mock-server tests cover successful posts, both reply argument shapes, the 403
error, tool selection, and absence of the credential from events, audit logs and the
workspace. They run in CI and need no X account:

```bash
cargo test -p orbit-worker
```

A live post is opt-in because it publishes a real tweet and needs a real user-context
token. Set up the app and token as above, then run:

```bash
ORBIT_X_LIVE=1 \
ORBIT_X_USER_TOKEN='<user-context token with tweet.write>' \
  cargo test -p orbit-worker --bin orbit-worker live_post_requires_user_context_token \
  -- --ignored --nocapture
```

The test posts a uniquely timestamped `ORBIT X connector live check …` message to the
token owner's account. Do not put the token in a committed file; export it in the shell
only. Media upload, threads, quote posts, polls and scheduling are out of scope.
