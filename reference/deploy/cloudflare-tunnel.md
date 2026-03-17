# Cloudflare Tunnel — Hetzner Migration

## Overview

Moving the Cloudflare Tunnel from Mac mini to Hetzner. The tunnel maps
`nodeapi.ai` → `http://localhost:8100` on whichever machine is running it.

---

## Step 1: On Hetzner — Authenticate cloudflared

```bash
cloudflared tunnel login
```

This opens a browser link — paste it on your Mac to authorize. Credentials
saved to `/root/.cloudflared/cert.pem`.

---

## Step 2: Find your existing tunnel ID

On Mac mini (or via Cloudflare dashboard → Zero Trust → Tunnels):

```bash
cloudflared tunnel list
```

Note the tunnel ID (UUID) for `nodeapi.ai`.

---

## Step 3: Move tunnel credentials to Hetzner

On Mac mini, find the credentials file:
```bash
ls ~/.cloudflared/*.json
# Will be: <TUNNEL_ID>.json
```

Copy it to Hetzner:
```bash
scp ~/.cloudflared/<TUNNEL_ID>.json nodeapi@<HETZNER_IP>:/home/nodeapi/.cloudflared/
```

---

## Step 4: Create tunnel config on Hetzner

```bash
mkdir -p /home/nodeapi/.cloudflared
cat > /home/nodeapi/.cloudflared/config.yml << EOF
tunnel: <TUNNEL_ID>
credentials-file: /home/nodeapi/.cloudflared/<TUNNEL_ID>.json

ingress:
  - hostname: nodeapi.ai
    service: http://localhost:8100
  - service: http_status:404
EOF
chown -R nodeapi:nodeapi /home/nodeapi/.cloudflared
```

---

## Step 5: Install cloudflared as systemd service

```bash
cloudflared service install
systemctl enable cloudflared
systemctl start cloudflared
```

---

## Step 6: Stop the tunnel on Mac mini

```bash
# On Mac mini:
launchctl stop com.cloudflare.cloudflared
launchctl unload ~/Library/LaunchAgents/com.cloudflare.cloudflared.plist
```

---

## Step 7: Verify

```bash
curl https://nodeapi.ai/v1/health
# Should return: {"status":"ok","version":"0.6.0"}
```

---

## Rollback

If something goes wrong, restart the Mac mini tunnel:
```bash
launchctl load ~/Library/LaunchAgents/com.cloudflare.cloudflared.plist
launchctl start com.cloudflare.cloudflared
```
And stop the Hetzner service:
```bash
systemctl stop cloudflared
```
