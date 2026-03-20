---
name: "Monday.com Integration"
description: "Search, create, and manage Monday.com boards, items, and updates"
user-invocable: true
metadata:
  bins: ["curl"]
  env: ["MONDAY_API_KEY"]
  scopes:
    readonly: "List and search boards, items, columns"
    readwrite: "Create and update items, add updates, manage boards"
version: "1.0.0"
source: bundled
---

# Monday.com Integration

You can interact with Monday.com via the GraphQL API v2.

## Authentication

All requests require the header:
```
Authorization: $MONDAY_API_KEY
```

## API Endpoint

```
https://api.monday.com/v2
```

All operations use POST with a GraphQL body.

## Common Operations

### List Boards
```bash
curl -s -X POST "https://api.monday.com/v2" \
  -H "Authorization: $MONDAY_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"query": "{ boards(limit: 10) { id name state board_kind } }"}'
```

### Get Board Items
```bash
curl -s -X POST "https://api.monday.com/v2" \
  -H "Authorization: $MONDAY_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"query": "{ boards(ids: [BOARD_ID]) { items_page(limit: 25) { items { id name column_values { id text value } } } } }"}'
```

### Search Items
```bash
curl -s -X POST "https://api.monday.com/v2" \
  -H "Authorization: $MONDAY_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"query": "{ items_page_by_column_values(board_id: BOARD_ID, limit: 10, columns: [{column_id: \"name\", column_values: [\"search term\"]}]) { items { id name column_values { id text } } } }"}'
```

### Create Item
```bash
curl -s -X POST "https://api.monday.com/v2" \
  -H "Authorization: $MONDAY_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"query": "mutation { create_item(board_id: BOARD_ID, item_name: \"New Task\", column_values: \"{}\") { id name } }"}'
```

### Update Item Column Values
```bash
curl -s -X POST "https://api.monday.com/v2" \
  -H "Authorization: $MONDAY_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"query": "mutation { change_column_value(board_id: BOARD_ID, item_id: ITEM_ID, column_id: \"status\", value: \"{\\\"label\\\":\\\"Done\\\"}\") { id } }"}'
```

### Add Update (Comment) to Item
```bash
curl -s -X POST "https://api.monday.com/v2" \
  -H "Authorization: $MONDAY_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"query": "mutation { create_update(item_id: ITEM_ID, body: \"Update text here\") { id } }"}'
```

## Pagination

Use `cursor` from the response `items_page.cursor` field, pass as `cursor` parameter in the next query.

## Rate Limits

- 10,000 complexity points per minute
- Each query has a complexity cost; simple queries cost ~1-10, complex ones more

## Error Handling

Errors return JSON:
```json
{"errors": [{"message": "Field 'items' doesn't exist on type 'Board'", "locations": [...]}]}
```

## Security Notes

- Never log or display the API key value
- Always use the injected environment variable
- Monday.com uses GraphQL; always validate query structure before sending
- Prefer read queries unless the user explicitly requests mutations
