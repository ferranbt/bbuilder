
# BBuilder

Bbuilder is a modular framework to deploy blockchain infrastructure using declarative configurations.

```
$ cargo run examples/input_ethereum.json
```

## Catalog

- ethereum
- polygon
- berachain

## Telemetry

BBuilder components automatically expose metrics for monitoring. To collect metrics from all running components:

```bash
just prometheus
```

This starts a Prometheus instance that automatically discovers and scrapes metrics from any Docker container labeled with `metrics:<port>`. The Prometheus UI is available at http://localhost:9090.
