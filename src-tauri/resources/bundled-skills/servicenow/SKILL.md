---
name: "ServiceNow Integration"
description: "Search and manage ServiceNow incidents, requests, and knowledge articles"
user-invocable: true
metadata:
  bins: ["curl"]
  env: ["SERVICENOW_INSTANCE_URL", "SERVICENOW_USERNAME", "SERVICENOW_PASSWORD"]
  scopes:
    readonly: "Search and read incidents, requests, knowledge articles"
    readwrite: "Create and update incidents, manage service requests"
version: "1.0.0"
source: bundled
---

# ServiceNow Integration

You can interact with ServiceNow via the Table API and other REST APIs.

## Authentication

ServiceNow uses HTTP Basic Auth:
```
Authorization: Basic $(echo -n "$SERVICENOW_USERNAME:$SERVICENOW_PASSWORD" | base64)
```

## Base URL

```
$SERVICENOW_INSTANCE_URL/api/now
```

The instance URL should be like `https://yourinstance.service-now.com` (no trailing slash).

## Common Operations

### List Incidents
```bash
curl -s -u "$SERVICENOW_USERNAME:$SERVICENOW_PASSWORD" \
  -H "Accept: application/json" \
  "$SERVICENOW_INSTANCE_URL/api/now/table/incident?sysparm_limit=10&sysparm_order_by=sys_updated_on&sysparm_order_direction=desc"
```

### Search Incidents
```bash
curl -s -u "$SERVICENOW_USERNAME:$SERVICENOW_PASSWORD" \
  -H "Accept: application/json" \
  "$SERVICENOW_INSTANCE_URL/api/now/table/incident?sysparm_query=short_descriptionLIKEsearch+term&sysparm_limit=10"
```

### Get Incident Details
```bash
curl -s -u "$SERVICENOW_USERNAME:$SERVICENOW_PASSWORD" \
  -H "Accept: application/json" \
  "$SERVICENOW_INSTANCE_URL/api/now/table/incident/{sys_id}"
```

### Create Incident
```bash
curl -s -X POST -u "$SERVICENOW_USERNAME:$SERVICENOW_PASSWORD" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json" \
  -d '{"short_description":"Issue title","description":"Detailed description","urgency":"2","impact":"2"}' \
  "$SERVICENOW_INSTANCE_URL/api/now/table/incident"
```

### Update Incident
```bash
curl -s -X PATCH -u "$SERVICENOW_USERNAME:$SERVICENOW_PASSWORD" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json" \
  -d '{"state":"2","work_notes":"Updated via NexiBot"}' \
  "$SERVICENOW_INSTANCE_URL/api/now/table/incident/{sys_id}"
```

### Search Knowledge Articles
```bash
curl -s -u "$SERVICENOW_USERNAME:$SERVICENOW_PASSWORD" \
  -H "Accept: application/json" \
  "$SERVICENOW_INSTANCE_URL/api/now/table/kb_knowledge?sysparm_query=short_descriptionLIKEsearch+term&sysparm_limit=10"
```

## Pagination

Use `sysparm_limit` and `sysparm_offset` parameters.

## Rate Limits

ServiceNow enforces rate limits per instance. Check `X-RateLimit-*` response headers.

## Error Handling

Errors return JSON:
```json
{"error": {"message": "Record not found", "detail": "..."}}
```

## Security Notes

- Never log or display passwords
- Always use the injected environment variables
- Instance URL should include the full `https://` prefix
- Prefer read operations unless the user explicitly requests writes
