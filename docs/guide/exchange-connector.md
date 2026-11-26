# Exchange on-premises connector (EWS)

The Exchange connector reads mail from an on-premises Exchange server through EWS. It is
for environments without Microsoft Graph; Exchange Online is not the target.

## Requirements

- Exchange Server reachable over **HTTPS**.
- EWS enabled, usually at `https://<server>/EWS/Exchange.asmx`.
- **Basic authentication** enabled for EWS and a dedicated service account.
- The certificate chain and hostname must validate. An internal CA can be supplied through
  the same global CA bundle as the rest of ORBIT (`SSL_CERT_FILE`,
  `REQUESTS_CA_BUNDLE` or `NODE_EXTRA_CA_CERTS`).
- A dedicated mailbox the service account may read. Do not use a personal mailbox.

## Create a connection

1. Open **Connections → New connection** and choose **Exchange on-premises**.
2. Enter the EWS URL and mailbox address.
3. Set `allowedFolders` (empty means `inbox` only) and optionally lower `maxResults` or
   `maxAttachmentBytes`.
4. Enter the credential as write-only JSON:

```json
{
  "domain": "EXAMPLE",
  "username": "svc-orbit",
  "password": "…"
}
```

`domain` is optional. When set and the username is not already `DOMAIN\user` or a UPN, ORBIT
uses `DOMAIN\user` for EWS Basic auth.

5. Allow only the capabilities the agent needs:
   - `mail.search`
   - `mail.get`
   - `mail.attachments.get`

Use **Test connection** to verify DNS/TLS, authentication and EWS access.

## Tool behaviour

- `mail.search` searches by subject substring in an allowed folder and returns message ids,
  subjects, senders, received times and previews.
- `mail.get` returns the text body, recipients and attachment metadata.
- `mail.attachments.get` returns the attachment as base64, capped by
  `maxAttachmentBytes`.

All capabilities are **read-only**. Sending mail and bounded mailbox changes are separate
follow-up slices; use the SMTP output for notifications.

## Security limits

- Basic authentication is sent only over HTTPS.
- NTLM and Kerberos are not implemented in this slice. A server that rejects Basic auth
  returns a readable authentication error rather than falling back to plaintext.
- The EWS host goes through ORBIT's outbound policy before every request.
- The worker receives the connection credential only for the run that uses it; it is never
  put in execution events or audit data.
- Incoming mail is untrusted input. Folder and mailbox allowlists are enforced by the
  connector, not by the agent prompt.
