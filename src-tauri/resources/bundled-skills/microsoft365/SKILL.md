---
name: "Microsoft 365 Integration"
description: "Search and manage Outlook mail, Calendar, OneDrive, and Teams messages"
user-invocable: true
metadata:
  bins: ["curl"]
  env: ["MICROSOFT365_CLIENT_ID", "MICROSOFT365_CLIENT_SECRET", "MICROSOFT365_TENANT_ID", "MICROSOFT365_REFRESH_TOKEN"]
  scopes:
    readonly: "Read mail, calendar events, files, and messages"
    readwrite: "Send mail, create events, upload files, post messages"
version: "1.0.0"
source: bundled
---

# Microsoft 365 Integration

You can interact with Microsoft 365 services via the Microsoft Graph API.

## Authentication

First, obtain an access token from the refresh token:
```bash
ACCESS_TOKEN=$(curl -s -X POST "https://login.microsoftonline.com/$MICROSOFT365_TENANT_ID/oauth2/v2.0/token" \
  -d "client_id=$MICROSOFT365_CLIENT_ID" \
  -d "client_secret=$MICROSOFT365_CLIENT_SECRET" \
  -d "refresh_token=$MICROSOFT365_REFRESH_TOKEN" \
  -d "grant_type=refresh_token" \
  -d "scope=https://graph.microsoft.com/.default" | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])")
```

Then use the access token:
```
Authorization: Bearer $ACCESS_TOKEN
```

## Base URL

```
https://graph.microsoft.com/v1.0
```

## Outlook Mail

### List Recent Messages
```bash
curl -s -H "Authorization: Bearer $ACCESS_TOKEN" \
  "https://graph.microsoft.com/v1.0/me/messages?\$top=10&\$orderby=receivedDateTime+desc&\$select=subject,from,receivedDateTime,bodyPreview"
```

### Search Messages
```bash
curl -s -H "Authorization: Bearer $ACCESS_TOKEN" \
  "https://graph.microsoft.com/v1.0/me/messages?\$search=%22search+term%22&\$top=10"
```

### Send Email
```bash
curl -s -X POST -H "Authorization: Bearer $ACCESS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"message":{"subject":"Subject","body":{"contentType":"Text","content":"Email body"},"toRecipients":[{"emailAddress":{"address":"recipient@example.com"}}]}}' \
  "https://graph.microsoft.com/v1.0/me/sendMail"
```

## Calendar

### List Upcoming Events
```bash
curl -s -H "Authorization: Bearer $ACCESS_TOKEN" \
  "https://graph.microsoft.com/v1.0/me/calendarview?\$top=10&startdatetime=$(date -u +%Y-%m-%dT%H:%M:%SZ)&enddatetime=$(date -u -v+7d +%Y-%m-%dT%H:%M:%SZ)"
```

### Create Event
```bash
curl -s -X POST -H "Authorization: Bearer $ACCESS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"subject":"Meeting","start":{"dateTime":"2024-01-15T10:00:00","timeZone":"America/Denver"},"end":{"dateTime":"2024-01-15T11:00:00","timeZone":"America/Denver"}}' \
  "https://graph.microsoft.com/v1.0/me/events"
```

## OneDrive

### List Recent Files
```bash
curl -s -H "Authorization: Bearer $ACCESS_TOKEN" \
  "https://graph.microsoft.com/v1.0/me/drive/recent"
```

### Search Files
```bash
curl -s -H "Authorization: Bearer $ACCESS_TOKEN" \
  "https://graph.microsoft.com/v1.0/me/drive/root/search(q='search+term')"
```

## Teams

### List Joined Teams
```bash
curl -s -H "Authorization: Bearer $ACCESS_TOKEN" \
  "https://graph.microsoft.com/v1.0/me/joinedTeams"
```

### List Channel Messages
```bash
curl -s -H "Authorization: Bearer $ACCESS_TOKEN" \
  "https://graph.microsoft.com/v1.0/teams/{teamId}/channels/{channelId}/messages?\$top=10"
```

## Pagination

Microsoft Graph uses `@odata.nextLink` in responses. Follow the URL for the next page.

## Rate Limits

- Per-app limits vary by API; typically 10,000 requests per 10 minutes
- Per-mailbox limits for mail: 10,000 requests per 10 minutes

## Error Handling

Errors return JSON:
```json
{"error": {"code": "InvalidAuthenticationToken", "message": "Access token has expired."}}
```

## Security Notes

- Never log or display client secret or refresh token
- Always use the injected environment variables
- Access tokens expire after ~1 hour; always refresh before use
- Prefer read operations unless the user explicitly requests writes
