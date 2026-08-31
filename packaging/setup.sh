#!/bin/sh
# Install co2ampel on this machine. Run on the Pi, from the directory `task
# deploy` copied here:
#
#   sudo ./setup.sh <user that should run the service>
set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: sudo $0 <user that should run the service>" >&2
    exit 2
fi
if [ "$(id -u)" -ne 0 ]; then
    echo "$0 must run as root (sudo)" >&2
    exit 2
fi

SERVICE_USER=$1
id "$SERVICE_USER" >/dev/null
HERE=$(dirname "$0")

install -m 0755 "$HERE/co2ampel" /usr/local/bin/co2ampel
install -m 0644 "$HERE/99-cleware.rules" /etc/udev/rules.d/99-cleware.rules
udevadm control --reload-rules
udevadm trigger

# An existing configuration is never overwritten — that file holds the broker
# credentials for this Pi.
CONFIGURED=yes
if [ ! -f /etc/default/co2ampel ]; then
    install -m 0640 "$HERE/co2ampel.env" /etc/default/co2ampel
    CONFIGURED=no
fi

# The unit ships with a placeholder user; bake in the one given here.
sed "s/^User=.*/User=$SERVICE_USER/" "$HERE/co2ampel.service" \
    > /etc/systemd/system/co2ampel.service
chmod 0644 /etc/systemd/system/co2ampel.service
systemctl daemon-reload

if [ "$CONFIGURED" = yes ]; then
    systemctl enable --now co2ampel.service
    echo "co2ampel installed and (re)started for user $SERVICE_USER."
else
    # Starting now would only loop against localhost:1883.
    systemctl enable co2ampel.service
    echo "co2ampel installed for user $SERVICE_USER, not started yet."
    echo "Set the broker and topic in /etc/default/co2ampel, then:"
    echo "  sudo systemctl start co2ampel.service"
fi
