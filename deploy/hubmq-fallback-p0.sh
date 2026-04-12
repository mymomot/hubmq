#!/usr/bin/env bash
# B1 — sends a direct email bypassing HubMQ itself when HubMQ is down.
set -euo pipefail

PASS_FILE="/etc/hubmq/credentials/gmail-app-password"
TO="${HUBMQ_FALLBACK_TO:-motreff@gmail.com}"
FROM="mymomot74@gmail.com"

HOSTNAME=$(hostname)
WHEN=$(date -Is)
STATUS=$(systemctl status hubmq --no-pager -n 20 2>&1 | head -40 || echo "systemctl status failed")

SUBJECT="[HubMQ P0 FALLBACK] hubmq.service failed on ${HOSTNAME}"
BODY="HubMQ service failed on ${HOSTNAME} at ${WHEN}.

This is a direct email fallback (bypass normal HubMQ pipeline).

systemctl status output:
${STATUS}

-- Sent by hubmq-fallback-p0.service (systemd OnFailure hook)"

PWD_VALUE=$(cat "${PASS_FILE}")

python3 <<PY
import smtplib, ssl
from email.message import EmailMessage
msg = EmailMessage()
msg["From"] = "${FROM}"
msg["To"] = "${TO}"
msg["Subject"] = """${SUBJECT}"""
msg.set_content("""${BODY}""")
ctx = ssl.create_default_context()
with smtplib.SMTP("smtp.gmail.com", 587, timeout=15) as s:
    s.starttls(context=ctx)
    s.login("${FROM}", "${PWD_VALUE}")
    s.send_message(msg)
print("FALLBACK_EMAIL_SENT_OK")
PY
