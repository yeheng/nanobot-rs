#!/bin/bash
# Migrate cron YAML files to markdown format

CRON_DIR="${HOME}/.gasket/cron"

if [ ! -d "$CRON_DIR" ]; then
    echo "Cron directory does not exist: $CRON_DIR"
    exit 0
fi

for yaml_file in "$CRON_DIR"/*.yaml; do
    if [ -f "$yaml_file" ]; then
        name=$(basename "$yaml_file" .yaml)
        md_file="$CRON_DIR/${name}.md"
        
        echo "Migrating $yaml_file -> $md_file"
        
        # Extract values from YAML
        cron=$(grep '^cron:' "$yaml_file" | sed 's/cron: *//' | tr -d '"')
        message=$(grep '^message:' "$yaml_file" | sed 's/message: *//')
        channel=$(grep '^channel:' "$yaml_file" | sed 's/channel: *//')
        to=$(grep '^to:' "$yaml_file" | sed 's/to: *//' | tr -d '"')
        enabled=$(grep '^enabled:' "$yaml_file" | sed 's/enabled: *//')
        
        # Create markdown file
        cat > "$md_file" << INNER_EOF
---
name: $name
cron: "$cron"
channel: $channel
to: "$to"
enabled: $enabled
---

$message
INNER_EOF
        
        echo "Migrated: $md_file"
    fi
done

echo "Migration complete!"
