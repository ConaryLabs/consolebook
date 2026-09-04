# Preview deployment

The demonstration site is <https://consolebook-preview.conary.io>. It contains
invented data and is separate from development installations. This is a host
runbook, not a production-readiness statement. Credentials belong outside the
repository; the browser first prompts for nginx Basic Auth, then Consolebook
sign-in.

## Installed layout

The setup recorded on 2026-09-02 uses:

| Component | Location or behavior |
| --- | --- |
| Service | `consolebook-preview.service`, enabled at boot, restarts on failure |
| Service account | `consolebook-preview`, no interactive login |
| Release executable | `/srv/consolebook-preview/bin/consolebook` |
| Persistent data and backups | `/srv/consolebook-preview/data/` |
| App listener | `127.0.0.1:7770` |
| nginx vhost | `/etc/nginx/sites-available/consolebook-preview`, linked in `sites-enabled/` |
| TLS certificate | `/etc/letsencrypt/live/consolebook-preview.conary.io/` |

nginx redirects HTTP to HTTPS, requires Basic Auth, proxies to the loopback
listener, adds HSTS and no-index headers, and forces `Secure` on the app session
cookie. The app itself does not yet set that attribute (ADR 0004).

The service runs a copied release executable. Editing the checkout, rebuilding
the web app, or merging a PR does not deploy it. There is no automatic deploy
pipeline or built-in Git revision display; the reported `0.0.0` cannot identify
which commit is running. Record the source commit and executable SHA-256 with
each deployment.

## Read-only status checks

On the host, inspect the service and listener:

```sh
systemctl status consolebook-preview.service --no-pager
systemctl cat consolebook-preview.service
ss -ltn | rg ':7770'
curl --fail http://127.0.0.1:7770/api/health
curl --head https://consolebook-preview.conary.io/
```

An unauthenticated HTTPS request should receive `401` with a Basic Auth
challenge. A local health response should report database `ok`; neither check
proves recovery or a complete user workflow. Logs are in the system journal;
first-run logs can contain the short-lived setup code, so inspect privately
and redact before sharing. Before using `doctor` on retained data, confirm the
installed binary includes the [#56](https://github.com/FieldmouseWorks/consolebook/issues/56)
repair: older binaries can change journal mode while diagnosing it. Repository
changes do not update that separately installed binary.
[ADR 0016](decisions/0016-read-only-diagnostics.md) defines the repaired command's
read-only database contract and WAL-sidecar limits.

## Updating the preview

Use the issue/branch/PR workflow in [CONTRIBUTING.md](../CONTRIBUTING.md) for
deployment changes. Check the current host configuration and data state first;
this runbook does not authorize replacing or reinitializing retained data.

1. Build and verify the chosen revision: web first, then Rust gates and browser
   tests, then `cargo build --release -p consolebook-server`.
2. Take and retain a validated backup using the installed binary and service
   account. Keep the old executable and record both source revision and hash.
3. Stage the new executable beside the installed one. Stop the preview service,
   install the staged executable with root ownership and executable permissions,
   then start the service. Startup may migrate the database.
4. Verify service state, loopback health, HTTPS authentication, and a browser
   sign-in through the public hostname. Record the outcome and revision.

An older binary is not necessarily compatible with a migrated database.
Recovery after a failed upgrade needs the saved binary and compatible snapshot;
do not assume a binary-only rollback is safe.

## Certificate renewal

The host uses Certbot's standalone authenticator. Existing global renewal hooks
stop nginx before renewal and start it afterward, briefly affecting all nginx
sites. A webroot challenge would fail with those hooks because its server is
stopped. Keep the authenticator and hooks consistent; do not change them for
one vhost without considering the other sites. Inspect the renewal config and
timer on the host; successful issuance alone does not prove a future renewal.

If Cloudflare proxying is enabled, use Full (strict) TLS to the origin. The
application remains on loopback; public port 7770 is unnecessary.
