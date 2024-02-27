hiddenlink is a simple tunnel which tries to hide itself inside of other protocols.

The main idea is the following: it has some transports configured which availability is constantly monitored. By default the fastest transport is used. If it becomes unavailable (blocked?), the next one (slower, but more inconspicuous) is used in the order of precedence.

For now, the following transports are supported:

1. Encrypted UDP: A simple protocol which sends each packet in a separate UDP datagram. Expected to be fast, but may reveal the tunnel as a suspicious point-to-point UDP connection with data which looks like an unreadable garbage.

2. HTTPS: hiddenlink listens to 443 port, terminates TLS connections and securely authenticates them. Authenticated connections are passed for tunnel data transfer and non-authenticated are proxied to a web server to make the traffic look like a regular HTTPS. Supports multiple domains (TLS certificates), allows to split the traffic into multiple connections, reopen them periodically and emulate uploading/downloading HTTP clients to make the connection not look like tunnel in many of its aspects.