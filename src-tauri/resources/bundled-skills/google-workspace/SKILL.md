---
name: "Google Workspace Integration"
description: "Search and manage Google Drive, Docs, Sheets, Calendar, and Gmail"
user-invocable: true
metadata:
  bins: ["curl"]
  env: ["GOOGLE_CLIENT_ID", "GOOGLE_CLIENT_SECRET", "GOOGLE_REFRESH_TOKEN"]
  scopes:
    readonly: "List and search files, read calendar events, read emails"
    readwrite: "Create and edit documents, manage calendar events, send emails"
version: "1.0.0"
source: bundled
---

# Google Workspace Integration

You can interact with Google Workspace APIs (Drive, Docs, Sheets, Calendar, Gmail).

## Authentication

Google Workspace uses OAuth 2.0. First, obtain an access token from the refresh token:

```bash
ACCESS_TOKEN=$(curl -s -X POST "https://oauth2.googleapis.com/token" \
  -d "client_id=$GOOGLE_CLIENT_ID" \
  -d "client_secret=$GOOGLE_CLIENT_SECRET" \
  -d "refresh_token=$GOOGLE_REFRESH_TOKEN" \
  -d "grant_type=refresh_token" | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])")
```

Then use the access token in all requests:
```
Authorization: Bearer $ACCESS_TOKEN
```

## Google Drive

### Search Files
```bash
curl -s -H "Authorization: Bearer $ACCESS_TOKEN" \
  "https://www.googleapis.com/drive/v3/files?q=name+contains+'search+term'&fields=files(id,name,mimeType,modifiedTime)"
```

### List Recent Files
```bash
curl -s -H "Authorization: Bearer $ACCESS_TOKEN" \
  "https://www.googleapis.com/drive/v3/files?orderBy=modifiedTime+desc&pageSize=10&fields=files(id,name,mimeType,modifiedTime)"
```

### Read File Content (Google Docs)
```bash
curl -s -H "Authorization: Bearer $ACCESS_TOKEN" \
  "https://docs.googleapis.com/v1/documents/{documentId}"
```

## Google Calendar

### List Upcoming Events
```bash
curl -s -H "Authorization: Bearer $ACCESS_TOKEN" \
  "https://www.googleapis.com/calendar/v3/calendars/primary/events?timeMin=$(date -u +%Y-%m-%dT%H:%M:%SZ)&maxResults=10&singleEvents=true&orderBy=startTime"
```

### Create Event
```bash
curl -s -X POST -H "Authorization: Bearer $ACCESS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"summary":"Meeting","start":{"dateTime":"2024-01-15T10:00:00-07:00"},"end":{"dateTime":"2024-01-15T11:00:00-07:00"}}' \
  "https://www.googleapis.com/calendar/v3/calendars/primary/events"
```

## Gmail

### List Messages
```bash
curl -s -H "Authorization: Bearer $ACCESS_TOKEN" \
  "https://gmail.googleapis.com/gmail/v1/users/me/messages?maxResults=10"
```

### Read Message
```bash
curl -s -H "Authorization: Bearer $ACCESS_TOKEN" \
  "https://gmail.googleapis.com/gmail/v1/users/me/messages/{messageId}?format=full"
```

## Pagination

Google APIs use `pageToken`. Check response for `nextPageToken` and pass it as `pageToken` parameter.

## Rate Limits

- Drive: 12,000 queries per minute
- Calendar: 500 queries per 100 seconds
- Gmail: 250 quota units per second per user

## Error Handling

Errors return JSON:
```json
{"error": {"code": 403, "message": "Rate Limit Exceeded", "errors": [...]}}
```

## Security Notes

- Never log or display client secret or refresh token values
- Always use the injected environment variables
- Access tokens expire after ~1 hour; always refresh before use
- Prefer read operations unless the user explicitly requests writes
