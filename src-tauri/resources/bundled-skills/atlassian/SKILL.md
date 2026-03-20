---
name: "Atlassian Integration"
description: "Search and manage Jira issues, Confluence pages, and projects"
user-invocable: true
metadata:
  bins: ["curl"]
  env: ["ATLASSIAN_EMAIL", "ATLASSIAN_API_TOKEN", "ATLASSIAN_DOMAIN"]
  scopes:
    readonly: "Search and read issues, pages, projects"
    readwrite: "Create and update issues, create and edit pages"
version: "1.0.0"
source: bundled
---

# Atlassian Integration (Jira + Confluence)

You can interact with Jira and Confluence via their REST APIs.

## Authentication

All requests use HTTP Basic Auth with the user's email and API token:
```
Authorization: Basic $(echo -n "$ATLASSIAN_EMAIL:$ATLASSIAN_API_TOKEN" | base64)
```

## Base URLs

- Jira: `https://$ATLASSIAN_DOMAIN.atlassian.net/rest/api/3`
- Confluence: `https://$ATLASSIAN_DOMAIN.atlassian.net/wiki/rest/api`

## Jira Operations

### Search Issues (JQL)
```bash
curl -s -u "$ATLASSIAN_EMAIL:$ATLASSIAN_API_TOKEN" \
  "https://$ATLASSIAN_DOMAIN.atlassian.net/rest/api/3/search?jql=project%3DPROJ+ORDER+BY+updated+DESC&maxResults=10"
```

### Get Issue Details
```bash
curl -s -u "$ATLASSIAN_EMAIL:$ATLASSIAN_API_TOKEN" \
  "https://$ATLASSIAN_DOMAIN.atlassian.net/rest/api/3/issue/{issueKey}"
```

### Create Issue
```bash
curl -s -X POST -u "$ATLASSIAN_EMAIL:$ATLASSIAN_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"fields":{"project":{"key":"PROJ"},"summary":"Issue title","issuetype":{"name":"Task"},"description":{"type":"doc","version":1,"content":[{"type":"paragraph","content":[{"type":"text","text":"Description"}]}]}}}' \
  "https://$ATLASSIAN_DOMAIN.atlassian.net/rest/api/3/issue"
```

### Update Issue Status
```bash
curl -s -X POST -u "$ATLASSIAN_EMAIL:$ATLASSIAN_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"transition":{"id":"21"}}' \
  "https://$ATLASSIAN_DOMAIN.atlassian.net/rest/api/3/issue/{issueKey}/transitions"
```

### List Projects
```bash
curl -s -u "$ATLASSIAN_EMAIL:$ATLASSIAN_API_TOKEN" \
  "https://$ATLASSIAN_DOMAIN.atlassian.net/rest/api/3/project"
```

## Confluence Operations

### Search Pages
```bash
curl -s -u "$ATLASSIAN_EMAIL:$ATLASSIAN_API_TOKEN" \
  "https://$ATLASSIAN_DOMAIN.atlassian.net/wiki/rest/api/content/search?cql=type%3Dpage+AND+text~%22search+term%22&limit=10"
```

### Get Page Content
```bash
curl -s -u "$ATLASSIAN_EMAIL:$ATLASSIAN_API_TOKEN" \
  "https://$ATLASSIAN_DOMAIN.atlassian.net/wiki/rest/api/content/{pageId}?expand=body.storage"
```

### Create Page
```bash
curl -s -X POST -u "$ATLASSIAN_EMAIL:$ATLASSIAN_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"type":"page","title":"Page Title","space":{"key":"SPACE"},"body":{"storage":{"value":"<p>Content</p>","representation":"storage"}}}' \
  "https://$ATLASSIAN_DOMAIN.atlassian.net/wiki/rest/api/content"
```

## Pagination

Jira uses `startAt` and `maxResults`. Confluence uses `start` and `limit`.

## Rate Limits

- Jira Cloud: Basic rate limits apply per user
- Confluence: Similar rate limits

## Error Handling

Errors return JSON:
```json
{"errorMessages": ["Issue does not exist"], "errors": {}}
```

## Security Notes

- Never log or display the API token
- Always use the injected environment variables
- The domain variable should not contain `https://` or `.atlassian.net`
- Prefer read operations unless the user explicitly requests writes
