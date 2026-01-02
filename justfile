# Build a crate's Docker image
build crate:
    docker build -t ghcr.io/ferranbt/bbuilder/{{crate}}:latest -f crates/{{crate}}/Dockerfile .

prometheus:
    docker run -d \
        --name prometheus \
        --user root \
        --network eth_test \
        -p 9095:9090 \
        -v $(pwd)/telemetry/prometheus.yml:/etc/prometheus/prometheus.yml \
        -v /var/run/docker.sock:/var/run/docker.sock \
        prom/prometheus:latest \
        --config.file=/etc/prometheus/prometheus.yml
