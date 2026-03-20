---
name: "Salesforce Integration"
description: "Search and manage Salesforce records, leads, opportunities, and cases"
user-invocable: true
metadata:
  bins: ["curl"]
  env: ["SALESFORCE_CLIENT_ID", "SALESFORCE_CLIENT_SECRET", "SALESFORCE_REFRESH_TOKEN", "SALESFORCE_INSTANCE_URL"]
  scopes:
    readonly: "Query and read records, leads, opportunities, cases"
    readwrite: "Create and update records, manage leads and opportunities"
version: "1.0.0"
source: bundled
---

# Salesforce Integration

You can interact with Salesforce via the REST API using OAuth 2.0.

## Authentication

First, obtain an access token from the refresh token:
```bash
ACCESS_TOKEN=$(curl -s -X POST "https://login.salesforce.com/services/oauth2/token" \
  -d "grant_type=refresh_token" \
  -d "client_id=$SALESFORCE_CLIENT_ID" \
  -d "client_secret=$SALESFORCE_CLIENT_SECRET" \
  -d "refresh_token=$SALESFORCE_REFRESH_TOKEN" | python3 -c "import sys,json; print(json.load(sys.stdin)['access_token'])")
```

Then use the access token:
```
Authorization: Bearer $ACCESS_TOKEN
```

## Base URL

```
$SALESFORCE_INSTANCE_URL/services/data/v59.0
```

## Common Operations

### SOQL Query (Search)
```bash
curl -s -H "Authorization: Bearer $ACCESS_TOKEN" \
  "$SALESFORCE_INSTANCE_URL/services/data/v59.0/query/?q=SELECT+Id,Name,Email+FROM+Contact+WHERE+Name+LIKE+'%25search%25'+LIMIT+10"
```

### Get Record
```bash
curl -s -H "Authorization: Bearer $ACCESS_TOKEN" \
  "$SALESFORCE_INSTANCE_URL/services/data/v59.0/sobjects/Account/{recordId}"
```

### List Recent Records
```bash
curl -s -H "Authorization: Bearer $ACCESS_TOKEN" \
  "$SALESFORCE_INSTANCE_URL/services/data/v59.0/recent"
```

### Create Record
```bash
curl -s -X POST -H "Authorization: Bearer $ACCESS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"Name":"Account Name","Industry":"Technology"}' \
  "$SALESFORCE_INSTANCE_URL/services/data/v59.0/sobjects/Account"
```

### Update Record
```bash
curl -s -X PATCH -H "Authorization: Bearer $ACCESS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"Description":"Updated description"}' \
  "$SALESFORCE_INSTANCE_URL/services/data/v59.0/sobjects/Account/{recordId}"
```

### Search (SOSL)
```bash
curl -s -H "Authorization: Bearer $ACCESS_TOKEN" \
  "$SALESFORCE_INSTANCE_URL/services/data/v59.0/search/?q=FIND+%7Bsearch+term%7D+IN+ALL+FIELDS+RETURNING+Account(Id,Name),Contact(Id,Name,Email)"
```

## Common Objects

- `Account` - Companies/organizations
- `Contact` - People
- `Lead` - Potential customers
- `Opportunity` - Deals/sales
- `Case` - Support cases
- `Task` - Activities

## Pagination

SOQL queries return `nextRecordsUrl` for pagination. Follow it to get more results.

## Rate Limits

- API requests per 24-hour period depend on your Salesforce edition
- Concurrent API request limit: 25

## Error Handling

Errors return JSON array:
```json
[{"message": "Session expired or invalid", "errorCode": "INVALID_SESSION_ID"}]
```

## Security Notes

- Never log or display client secret or refresh token
- Always use the injected environment variables
- Access tokens expire; always refresh before use
- Prefer read operations (SOQL queries) unless the user explicitly requests writes
