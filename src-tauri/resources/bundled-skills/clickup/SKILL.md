---
name: "ClickUp Integration"
description: "Search, create, and manage ClickUp tasks, spaces, and lists"
user-invocable: true
metadata:
  bins: ["curl"]
  env: ["CLICKUP_API_KEY"]
  scopes:
    readonly: "List and search tasks, spaces, lists"
    full: "Create, update, delete tasks and manage workspaces"
version: "1.0.0"
source: bundled
---

# ClickUp Integration

You can interact with the user's ClickUp workspace via the ClickUp API v2.

## Authentication

All requests require the header:
```
Authorization: $CLICKUP_API_KEY
```

## Base URL

```
https://api.clickup.com/api/v2
```

## Common Operations

### List Teams/Workspaces
```bash
curl -s -H "Authorization: $CLICKUP_API_KEY" \
  "https://api.clickup.com/api/v2/team"
```

### List Spaces in a Workspace
```bash
curl -s -H "Authorization: $CLICKUP_API_KEY" \
  "https://api.clickup.com/api/v2/team/{team_id}/space"
```

### List Lists in a Space
```bash
curl -s -H "Authorization: $CLICKUP_API_KEY" \
  "https://api.clickup.com/api/v2/space/{space_id}/list"
```

### Get Tasks in a List
```bash
curl -s -H "Authorization: $CLICKUP_API_KEY" \
  "https://api.clickup.com/api/v2/list/{list_id}/task"
```

### Search Tasks
```python
import os, json, urllib.request
api_key = os.environ["CLICKUP_API_KEY"]
team_id = "TEAM_ID"  # Get from list teams first
query = "search terms"
url = f"https://api.clickup.com/api/v2/team/{team_id}/task?name={query}"
req = urllib.request.Request(url, headers={"Authorization": api_key})
resp = urllib.request.urlopen(req)
tasks = json.loads(resp.read())
for task in tasks.get("tasks", []):
    print(f"- [{task['status']['status']}] {task['name']} (ID: {task['id']})")
```

### Create a Task
```bash
curl -s -X POST \
  -H "Authorization: $CLICKUP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name": "Task name", "description": "Task description", "status": "to do"}' \
  "https://api.clickup.com/api/v2/list/{list_id}/task"
```

### Update a Task
```bash
curl -s -X PUT \
  -H "Authorization: $CLICKUP_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"status": "in progress"}' \
  "https://api.clickup.com/api/v2/task/{task_id}"
```

## Pagination

ClickUp uses `page` parameter (0-indexed). Response includes tasks array. If you get the maximum number of tasks, increment page and fetch again.

## Rate Limits

- 100 requests per minute per API key
- If you receive HTTP 429, wait and retry

## Error Handling

Errors return JSON with an `err` field:
```json
{"err": "Token invalid", "ECODE": "OAUTH_023"}
```

## Security Notes

- Never log or display the API key value
- Always use the injected environment variable
- Prefer read operations unless the user explicitly requests writes
