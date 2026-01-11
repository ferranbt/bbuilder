# Fetcher

A lightweight tool to fetch files from HTTP/HTTPS sources with progress tracking, checksum verification, and automatic extraction of tar.gz archives.

## WebSocket Progress Server

Enable the WebSocket server with `--ws` to stream real-time progress updates via JSON-RPC (default: `127.0.0.1:7070`).

```bash
echo '{"jsonrpc":"2.0","method":"subscribeProgress","params":[],"id":1}' | websocat ws://127.0.0.1:7070
```
