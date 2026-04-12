# NATS NKeys — HubMQ

## Storage (sur LXC 415, NE PAS versionner)

- `/etc/nats/nkeys/hubmq-service.seed` — service daemon (chmod 600 nats:nats)
- `/etc/nats/nkeys/hubmq-service.pub` — public key
- `/etc/nats/nkeys/publisher.seed` — publishers (Wazuh, Forgejo, BigBrother, cron)
- `/etc/nats/nkeys/publisher.pub` — public key

## Permissions (dans nats-server.conf)

### hubmq-service (daemon)
Full access publish/subscribe sur tous subjects.

### publisher (sources externes)
- publish: alert.>, monitor.>, agent.>, system.>, cron.>, user.incoming.>
- subscribe: _INBOX.> (necessaire pour request/reply)

## Distribution publisher seed

Le seed `publisher.seed` peut etre distribue aux sources externes (Wazuh webhook, Forgejo CI, BigBrother) via :
- systemd LoadCredential (prefere)
- ou fichier chmod 600 root:root

Jamais commite, jamais expose en env var.

## Logfile

`/var/log/nats-server.log` — owner nats:nats (chmod 640).
Creer manuellement si absent : `sudo touch /var/log/nats-server.log && sudo chown nats:nats /var/log/nats-server.log`

## Rotation

Pour regenerer :
1. `sudo /usr/local/bin/nsc generate nkey --user` sur LXC 415
2. Mettre a jour `/etc/nats/nkeys/hubmq-service.pub` ou `publisher.pub`
3. Mettre a jour `/etc/nats/nats-server.conf` avec la nouvelle cle publique
4. `sudo systemctl restart nats`
5. Distribuer le nouveau seed aux consumers concernes
