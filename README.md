# Libretto

[![Chat on Matrix](https://img.shields.io/matrix/libretto%3Aellis.link?server_fqdn=matrix.ellis.link&fetchMode=summary&logo=matrix)](https://matrix.to/#/#libretto:ellis.link?via=ellis.link&via=maunium.net)

A modern Matrix chat archiver that creates portable, searchable HTML archives of Matrix room conversations.

## What is Libretto?

Libretto is a Matrix bot that joins rooms and creates self-contained HTML archives of chat history. Additionally, rather than relying on the Matrix server being up, Libretto stores a copy of everything in a PostgreSQL database.

Originally developed as part of Google Summer of Code 2025 for MetaBrainz.

## Quick Start

### Prerequisites

- PostgreSQL database
- Matrix account credentials
- Only when building from source:
    - Rust
    - Node.js and pnpm (for asset building)

### Using Docker

1. ```bash
   docker pull ghcr.io/jadedblueeyes/libretto:main
   ```

2. ```bash
   cp config.example.toml config.toml
   # Edit config.toml with your settings
   ```

3. **Run with Docker Compose**:
   ```bash
   docker run --rm -v libretto:/libretto:z,U -v libretto-cache:/libretto-cache:z,U -v ./config.toml:/config.toml:ro -e CONFIG_FILE=/config.toml -e MATRIX_DATA_DIR=/libretto -e MATRIX_CACHE_DIR=/libretto-cache -e DATABASE_URL="postgres://user:password@host:4664/database" ghcr.io/jadedblueeyes/libretto:main
   ```

## Environment Variables

- `DATABASE_URL`: PostgreSQL connection string
- `DATA_DIR`: Directory for HTML files and media (optional)
- `CACHE_DIR`: Directory for Matrix SDK cache (optional)
- `SEARCH_INDEX_DIR`: Directory for the search index (optional, unused)

## Development

### Project Structure

```
libretto/
├── src/        # Rust source code
├── templates/  # Askama HTML templates
├── css/        # Stylesheet source
├── js/         # JavaScript source
├── migrations/ # Database migrations
└── dist/       # Built web assets
```

### Building Assets

```bash
# Development build with watching
pnpm watch

# Production build
pnpm build
```

## Limitations & Future Work

### Current Limitations

- **No full-text search**: Room-level filtering only (search was an optional feature)
- **Basic threading**: Threads are not separated into their own timelines
- **No admin controls**: Bot must be puppeted to join rooms

### Planned Improvements

- Full-text search with Tantivy
- Proper threaded conversation UI
- Admin user whitelist to automatically follow invites
- Operational monitoring (Prometheus/Sentry)
- Advanced media handling (thumbnails, etc.)
- Automatic history backfill
- State for member profiles
- Aggregate reactions
