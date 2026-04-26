# Dockerfile for systemd-journal-gatewayd testing
# Invokes daemons directly without full systemd
# syntax=docker/dockerfile:1.4

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y systemd-journal-remote && \
    apt-get clean && \
    rm -rf /var/lib/apt/lists/*

RUN mkdir -p /var/log/journal /run/systemd

EXPOSE 19531

# Run journal-gatewayd directly (not via systemd)
# It listens on port 19531 for HTTP connections
# Note: requires journal files to exist, or --system may fail
RUN cat >/entrypoint.sh <<EOF
#!/usr/bin/env bash

/usr/lib/systemd/systemd-journald &
/usr/lib/systemd/systemd-journal-gatewayd
"$@"
EOF

RUN chmod +x /entrypoint.sh

ENTRYPOINT ["/entrypoint.sh"]
