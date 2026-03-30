#!/bin/bash
# SSL setup for greed-compute via Let's Encrypt
# Run after DNS is pointing to this VPS
# Usage: bash ssl-setup.sh your@email.com
set -e

EMAIL="${1:-}"
DOMAIN="compute.deep-ml.com"

if [ -z "$EMAIL" ]; then
    echo "Usage: bash ssl-setup.sh your@email.com"
    exit 1
fi

echo "Setting up SSL for $DOMAIN..."
echo ""

# Verify DNS is pointing here before attempting cert
CURRENT_IP=$(curl -s https://api.ipify.org)
DNS_IP=$(dig +short "$DOMAIN" | tail -1)

echo "This VPS IP : $CURRENT_IP"
echo "DNS resolves: $DNS_IP"
echo ""

if [ "$CURRENT_IP" != "$DNS_IP" ]; then
    echo "WARNING: DNS is not yet pointing to this VPS."
    echo "Update your A record for $DOMAIN to $CURRENT_IP, wait for propagation, then re-run."
    echo ""
    read -p "Continue anyway? (y/N): " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
fi

certbot --nginx \
    -d "$DOMAIN" \
    --non-interactive \
    --agree-tos \
    -m "$EMAIL" \
    --redirect

echo ""
echo "SSL configured. nginx will now serve HTTPS on port 443."
echo "Auto-renewal is handled by certbot systemd timer:"
systemctl status certbot.timer --no-pager
echo ""
echo "Test: curl https://$DOMAIN/v1/health"
