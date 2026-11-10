# Running the Annulus sync server

Last verified: 2026-11-08 @ 51a7d79

The sync server keeps your hosts and rules in step across your devices. It stores only
ciphertext: everything is encrypted on your devices with your master password, and login
uses OPAQUE, so the server never sees the password ([ADR-0008](../adr/0008-self-hosted-sync-server.md)).
One server can hold several separate accounts (a family, a homelab); accounts never see
each other's data.

## Quick start with Docker and automatic HTTPS

You need a machine reachable from your devices, a DNS name pointing at it, and ports 80
and 443 open. Files are in [`deploy/`](../../deploy/).

1. In `deploy/Caddyfile`, replace `sync.example.com` with your name.
2. Start it: `docker compose -f deploy/compose.yaml up -d`. Sign-ups stay closed.
3. Make an invite code for your account (see [Invites](#invites)):
   `docker compose -f deploy/compose.yaml exec annulus annulus-server invite --data /data/annulus-server.db`
4. In the app, open **Settings → Account & sync**, enter `https://<your name>`, an account
   name, your master password and the invite code, and choose **Create account**. Write
   down nothing new: the recovery key you saved when you created the vault also recovers
   the account.
5. On other devices, choose **Sign in** with the same account and password (no invite
   needed). Hosts already on such a device are merged into the account; its old vault
   file is kept next to the new one as `vault.before-sync-<time>.anv`.

Caddy obtains and renews the certificate. The server itself listens on plain HTTP inside
the container network only, in a read-only container without capabilities; only the
database volume is writable.

## On a home network, with the server's own certificate

With no DNS name a public authority will certify, give the server its own certificate and
let each device trust it by its fingerprint ([ADR-0018](../adr/0018-pinned-server-certificate.md)):

```text
annulus-server tls-init --name 192.0.2.20 --out /etc/annulus
annulus-server serve --listen 0.0.0.0:8443 --data /var/lib/annulus/server.db \
               --tls-cert /etc/annulus/cert.pem --tls-key /etc/annulus/key.pem
```

`--name` takes the address (or names, comma-separated) the devices will use. `tls-init`
prints the certificate's fingerprint; `annulus-server fingerprint --tls-cert cert.pem`
prints it again. The first time a device signs in or creates the account, the app shows
the fingerprint it sees: choose **Trust this certificate** only if the two match. From
then on that certificate is the only one the device accepts for the server, so a different
one stops sync instead of being trusted. A new certificate means signing out and in again
on each device.

## Running the binary directly

```text
annulus-server serve --listen 0.0.0.0:443 --data /var/lib/annulus/server.db \
               --tls-cert fullchain.pem --tls-key privkey.pem [--signups open]
```

- Without a certificate the server serves plain HTTP on loopback only (for a proxy on the
  same machine, or testing). Anywhere else it needs `--tls-cert`/`--tls-key`, or
  `--behind-proxy` when a TLS proxy forwards to it.
- `--behind-proxy` also makes the server read the client's address from the proxy's
  `X-Forwarded-For` header, for the per-address login limit. A proxy on loopback is trusted.
  Name each other proxy by its exact address with a repeatable `--trusted-proxy ADDRESS`.
  The server walks the header from the right past only those trusted addresses, so a client
  on the same private network cannot choose its own address. Without `--behind-proxy`, the
  header is ignored and the connection's address counts.
- `--signups closed` is the default: an account can be created only with an invite code.
  `--signups open` lets anyone who can reach the server create an account; use it only on
  a trusted network.
- The app accepts `https://` addresses, and `http://` only for a server on the same
  computer. A certificate not from a public authority is trusted only after the user
  compares its fingerprint (see above).

## Invites

While sign-ups are closed, each new account needs an invite code from the admin:

```text
annulus-server invite [--expires 24h] --data <db>
```

The command prints a code such as `1a2b-3c4d-5e6f-7a8b-9c0d-1e2f-3a4b-5c6d`. Give it to the
person creating the account (yourself, the first time); they enter it under **Invite code**
when they choose **Create account** in the app. A code:

- creates one account, then stops working; a refused account name does not use it up;
- expires after `--expires` (minutes, hours or days, such as `90m`, `24h` or `7d`; the
  default is 24 hours, at most 30 days);
- is stored only as a hash, like device tokens;
- is not needed to sign in to an existing account, or while sign-ups are open.

The command works while the server runs. With Docker, run it inside the container as
shown in the quick start.

## Accounts and devices

- Each signed-in device has its own token. A token that is not used for 90 days stops
  working, and the device has to sign in again.
- Changing the master password in the app proves the current password to the server
  first; a stolen device token alone cannot change it. The change signs every other
  device out.
- Failed and unfinished logins are limited per account and per client address; a paused
  account or address waits 15 minutes.

## Administration

| Command | What it does |
|---|---|
| `annulus-server accounts --data <db>` | Lists accounts with their device and record counts and stored bytes. |
| `annulus-server delete-account <name> --data <db>` | Deletes an account with its devices, records and history. |
| `annulus-server backup <file> --data <db>` | Writes a consistent copy while the server runs. |
| `annulus-server invite [--expires 24h] --data <db>` | Prints a single-use invite code for one new account ([Invites](#invites)). |

To restore, stop the server and put the backup in place of the database file. The backup
holds the server's OPAQUE key, so accounts sign in as before. With Docker, run the
commands inside the container, e.g.
`docker compose -f deploy/compose.yaml exec annulus annulus-server backup /data/backup.db --data /data/annulus-server.db`.

## Monitoring

`GET /healthz` answers `ok` to anyone, without signing in, for a container health check
or an uptime monitor. The server writes only its start-up lines (address, certificate
fingerprint) to standard error. It logs no requests, bodies, tokens or account names.

Behind a proxy, give requests at least 30 seconds before the proxy times them out:
devices wait up to 25 seconds on `/v1/records/wait` to hear of changes at once. Caddy's and
nginx's defaults are long enough. The server itself ends any request after 60 seconds.
When it serves TLS itself, it also drops a connection that has not sent its headers
within 10 seconds; behind a proxy, the proxy does that.

## What the server stores and sees

See the table in [ADR-0008](../adr/0008-self-hosted-sync-server.md#what-the-server-stores-and-what-it-can-see):
account names, device names, the number, sizes and times of records, and nothing about
their contents. Each record keeps its last 20 versions; deletions are kept as tombstones,
with whatever sealed data the app sends for them.

## Upgrading

Stop the server, take a backup, replace the binary or image, start it again. The
database schema is created and extended automatically.

## Not yet

A 30-day limit on deleted records and history (today the last 20 versions of each record
are kept, and deletions are kept until the account goes), and a per-account storage quota,
are planned.
