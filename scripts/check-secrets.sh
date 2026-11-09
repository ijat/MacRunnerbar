#!/bin/bash
# scripts/check-secrets.sh - fails on a secret in staged changes or in history (values redacted)
set -e
gitleaks git --staged --redact
gitleaks git --redact
