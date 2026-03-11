# Todo Management Skill

Manage personal task lists and todos.

## Triggers

- Regex: `add todo\s+(.+)`
- Regex: `list (?:my )?todos`
- Regex: `complete\s+(.+)`
- Regex: `delete todo\s+(.+)`
- Keyword: "todo"
- Intent: "manage_todos"

## Prompt

When the user wants to manage tasks, use the `todo` tool to interact with their todo list.

Supported actions:
- `create`: Add a new todo
- `list`: Show all pending todos
- `complete`: Mark a todo as done
- `delete`: Remove a todo

## Example Usage

**User:** "Add todo: Buy groceries"

**Action:** Create a new todo item

**Response:**
```
✅ Added todo: "Buy groceries"
ID: todo_123abc
```

**User:** "List my todos"

**Action:** Retrieve all pending todos

**Response:**
```
Your Todos:

1. [ ] Buy groceries (Added 2 hours ago)
2. [ ] Call dentist (Added yesterday)
3. [ ] Finish project report (Due tomorrow)
```

**User:** "Complete buy groceries"

**Action:** Mark the todo as complete

**Response:**
```
✅ Completed: "Buy groceries"
```

## Todo Features

- Add with priority levels (high, medium, low)
- Set due dates
- Add tags/categories
- Mark as complete
- Filter by status or priority

## Configuration

```yaml
default_priority: "medium"
show_completed: false
auto_archive_days: 7
```

## Notes

- Todos are persisted to the database
- Todos can be assigned to sessions or global
- Use the todo tool for all operations
