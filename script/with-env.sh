#!/usr/bin/env bash
# Runs a command with the settings in .env. The file is read as data and is not executed.
# A value already in the environment wins, and the origin and the key stay together as a pair.
set -euo pipefail

env_file=${RONNIE_ENV_FILE:-.env}
service_set=${RONNIE_OAUTH_ORIGIN+set}${RONNIE_OAUTH_CLIENT_ID+set}

if [ -f "$env_file" ]; then
    while IFS= read -r line || [ -n "$line" ]; do
        # A .env file from before the rename to Ronnie uses the prefix RONIE_.
        if [[ ! "$line" =~ ^RONN?IE_([A-Z_]+)=(.+)$ ]]; then
            continue
        fi
        name=RONNIE_${BASH_REMATCH[1]}
        value=${BASH_REMATCH[2]}
        if [[ "$line" == RONIE_* ]]; then
            printf 'warning: %s uses the old name RONIE_%s; rename it to %s\n' \
                "$env_file" "${BASH_REMATCH[1]}" "$name" >&2
        fi
        case "$name" in
        RONNIE_OAUTH_ORIGIN | RONNIE_OAUTH_CLIENT_ID)
            [ -z "$service_set" ] || continue
            ;;
        esac
        if [ -z "${!name+set}" ]; then
            export "$name=$value"
        fi
    done <"$env_file"
fi

exec "$@"
