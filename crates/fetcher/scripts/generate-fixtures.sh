#!/bin/bash
set -e

# Navigate to the fetcher directory
cd "$(dirname "$0")/.."

echo "Creating content.tar.gz from content.txt..."
tar -czf fixtures/content.tar.gz -C fixtures content.txt

echo "Generating checksums..."

# Calculate SHA256 hashes
content_hash=$(shasum -a 256 fixtures/content.txt | cut -d' ' -f1)
archive_hash=$(shasum -a 256 fixtures/content.tar.gz | cut -d' ' -f1)

# Create JSON file with checksums
cat > fixtures/checksums.json <<EOF
{
  "content.txt": "${content_hash}",
  "content.tar.gz": "${archive_hash}"
}
EOF

echo "✓ Created fixtures/content.tar.gz"
echo "✓ Created fixtures/checksums.json"
echo ""
echo "Checksums:"
echo "  content.txt:     ${content_hash}"
echo "  content.tar.gz:  ${archive_hash}"
