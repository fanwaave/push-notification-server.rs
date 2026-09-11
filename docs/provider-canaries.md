# Provider canaries

The normal test suite uses deterministic mock provider endpoints and real process/JetStream transports. It never needs production credentials and never contacts SendGrid, Twilio, Apple, Google, Expo, or browser push services.

For an additional provider-contract check, the `Provider canaries` workflow can be started manually after repository secrets are configured. A dispatched workflow is an evidence-producing operation: missing credentials fail the applicable job instead of producing a misleading successful skip.

## SendGrid sandbox canary

Required managed secrets:

- `SENDGRID_CANARY_API_KEY`: a dedicated Custom Access key with exactly the `mail.send` scope
- `SENDGRID_CANARY_FROM_EMAIL`: a verified sender
- `SENDGRID_CANARY_TO_EMAIL`: a syntactically valid canary recipient

Optional repository variable:

- `SENDGRID_CANARY_REGION`: `global` or `eu`; empty defaults to `global`

The test first authenticates the key through `GET /v3/scopes` and rejects Full Access or any mixed-purpose scope set. It then uses SendGrid sandbox mode, which validates the Mail Send payload without delivering email, consuming delivery credits, or emitting Email Activity/Event Webhook activity.

The canary API key must not be copied from another service or environment. Rotate it independently and revoke it after any suspected exposure.

## Twilio test-credential canary

Required managed secrets:

- `TWILIO_TEST_ACCOUNT_SID`
- `TWILIO_TEST_AUTH_TOKEN`
- `TWILIO_TEST_TO_NUMBER`

The test uses Twilio's documented valid magic From number with account test credentials. Twilio returns a realistic Messages API response without billing, mutating production state, or connecting to a carrier.

Test credentials are deliberately separate from the production credential mode. Production should use a service-specific API Key rather than the primary Account Auth Token; where the selected Twilio product and region support it, use a Restricted API Key scoped only to the required Messaging endpoints.

## Safety and evidence

- The workflow is `workflow_dispatch` only; it is not run for arbitrary pull requests.
- A dispatch with an incomplete provider bundle fails and lists variable names only.
- Credentials are supplied only through GitHub Actions secrets; no value belongs in workflow inputs, repository files, `env/dec`, logs, traces, or tickets.
- Provider secret bytes are not trimmed or normalized. Whitespace and non-printable bytes fail before a request.
- Test jobs assert that normalized outcomes contain fingerprints rather than recipient addresses or phone numbers.
- These canaries prove authentication, least-privilege SendGrid scope, and provider request compatibility. They do not prove final delivery. Signed provider callbacks and durable reconciliation are tracked separately.
