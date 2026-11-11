#!/usr/bin/env bash
# Runs a command with the settings in .env. The file is read as data and is not executed.
# A value already in the environment wins, and the origin and the key stay together as a pair.
set -euo pipefail

env_file=${RONIE_ENV_FILE:-.env}
service_set=${RONIE_OAUTH_ORIGIN+set}${RONIE_OAUTH_CLIENT_ID+set}

if [ -f "$env_file" ]; then
    while IFS= read -r line || [ -n "$line" ]; do
        if [[ ! "$line" =~ ^(RONIE_[A-Z_]+)=(.+)$ ]]; then
            continue
        fi
        name=${BASH_REMATCH[1]}
        value=${BASH_REMATCH[2]}
        case "$name" in
        RONIE_OAUTH_ORIGIN | RONIE_OAUTH_CLIENT_ID)
            [ -z "$service_set" ] || continue
            ;;
        esac
        if [ -z "${!name+set}" ]; then
            export "$name=$value"
        fi
    done <"$env_file"
fi

exec "$@"
