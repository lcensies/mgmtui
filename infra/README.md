# infra — production deployment for `mgmt web`

Two layers, same stack:

- **`docker/`** — a standalone Docker Compose stack. Clone, fill `.env`, `docker compose up -d`.
  Optional bundled **Traefik** that gets **Let's Encrypt** certs over the **DNS-01** challenge
  (works behind NAT, supports wildcards).
- **`ansible/`** — a `mgmt_web` role that installs Docker and deploys that stack to a host,
  **tested in isolation with Molecule** (docker-in-docker).

The image is a single self-contained binary: `mgmt web` (axum API) with the PWA baked in
(`--features embed-ui`), on a slim non-root Debian runtime. Config + state live in one `/data`
volume (`web-auth.yaml` — password/TOTP/tokens — persists there too).

## Quick start (Docker only)

```bash
cd infra/docker
cp .env.example .env          # set MGMT_WEB_PASSWORD, MGMT_DOMAIN, ACME_EMAIL, DNS creds
# no reverse proxy — publishes 127.0.0.1:8321 for your own Caddy/nginx:
docker compose up -d --build
# …or with automatic HTTPS via Traefik + Let's Encrypt (DNS-01):
COMPOSE_PROFILES=traefik docker compose up -d --build
```

Then open `https://<MGMT_DOMAIN>` and "Add to Home Screen". The password is seeded from
`MGMT_WEB_PASSWORD` on first boot; add 2FA later with
`docker compose exec mgmt-web mgmt web totp-enroll`.

### TLS / DNS-01

Traefik requests certs by proving DNS control, so **no inbound :80 is required to issue** and you
can get wildcards. Pick your provider's [lego code](https://go-acme.github.io/lego/dns/) in
`TRAEFIK_DNS_PROVIDER` and fill only that provider's credential vars in `.env` (Cloudflare,
Route 53, DigitalOcean, and Gandi are pre-wired; add more env passthroughs in
`docker-compose.yml` if needed).

> Start with `ACME_CA_SERVER` pointed at **staging** (the default in `.env.example`) to dodge
> Let's Encrypt's prod rate limits, confirm a cert is issued, then flip to prod and
> `docker compose up -d` again.

## Ansible deployment

```bash
cd infra/ansible
ansible-galaxy collection install -r requirements.yml
cp inventory/hosts.example.yml inventory/hosts.yml     # edit host + vars
ansible-vault create group_vars/mgmt_web/vault.yml     # mgmt_web_password, mgmt_web_dns_env
ansible-playbook site.yml -i inventory/hosts.yml --ask-vault-pass
```

Key role variables (`roles/mgmt_web/defaults/main.yml`):

| var | default | meaning |
|---|---|---|
| `mgmt_web_password` | — | **required**, vault it — web login password |
| `mgmt_web_reverse_proxy` | `traefik` | `traefik` or `none` (bring your own proxy) |
| `mgmt_web_domain` / `mgmt_web_acme_email` | example.com | host + ACME contact |
| `mgmt_web_dns_provider` / `mgmt_web_dns_env` | cloudflare / `{}` | DNS-01 provider + its creds |
| `mgmt_web_source` | `""` | host path to a repo checkout to build from; empty ⇒ use a prebuilt `mgmt_web_image` |
| `mgmt_web_install_docker` | `true` | install Docker Engine + compose plugin |

## Testing the role (Molecule)

```bash
pipx install molecule 'molecule-plugins[docker]' ansible-core
cd infra/ansible/roles/mgmt_web
molecule test            # create → converge → idempotence → verify → destroy
```

The scenario boots a systemd-capable container, has the role install Docker inside it, and brings
a **stub** stack up (`traefik/whoami`, `reverse_proxy: none`) — so it verifies the deployment
mechanics (Docker install, `.env`/compose rendering, `docker compose config` validity, container
running, idempotence) fast and hermetically, **without** compiling the Rust image. That heavy
build is covered by `docker build` / CI. Requires a working Docker daemon and outbound network on
the test host.
