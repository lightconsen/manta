# Skill Template

This directory contains a template for creating new Manta skills.

## Quick Start

1. Copy this directory to a new skill folder:
   ```bash
   cp -r examples/skills/_template examples/skills/my_new_skill
   ```

2. Edit `SKILL.md` and fill in all the `{{placeholders}}`

3. Test your skill by loading it into Manta

## File Structure

```
my_new_skill/
├── SKILL.md        # Skill definition (required)
├── config.yaml     # Optional configuration
└── README.md       # Optional documentation
```

## SKILL.md Sections

### Required

- **Title** - Name of the skill
- **Triggers** - How the skill is activated
- **Prompt** - Additional instructions for the LLM

### Optional

- **Configuration** - Custom settings
- **Examples** - Sample interactions
- **Tools** - Required tools
- **Notes** - Additional information

## Trigger Types

### Keyword Trigger
Simple word matching:
```markdown
## Triggers
- Keyword: "weather"
- Keyword: "forecast"
```

### Regex Trigger
Pattern matching for complex triggers:
```markdown
## Triggers
- Regex: `weather (?:in|for)?\s*(.+)`
- Regex: `temperature (?:in|for)?\s*(.+)`
```

### Intent Trigger
NLP-based intent detection:
```markdown
## Triggers
- Intent: "get_weather"
- Intent: "check_forecast"
```

## Best Practices

1. **Be Specific** - Clear triggers prevent false activations
2. **Provide Examples** - Helps the LLM understand usage
3. **Document Tools** - List required tools explicitly
4. **Keep it Focused** - One skill = one responsibility
5. **Test Thoroughly** - Verify with various inputs

## Example: Complete Skill

See the `weather` skill for a complete example:
```markdown
# Weather Skill

Fetch weather information for any location.

## Triggers

- Regex: `weather (?:in|for)?\s*(.+)`
- Keyword: "weather"
- Intent: "get_weather"

## Prompt

When the user asks about weather:
1. Extract the location from the query
2. Use web_search to find current weather
3. Format the response with temperature, conditions, and forecast

## Tools

- `web_search` - Find weather data

## Example

**User:** "What's the weather in Paris?"

**Response:**
```
🌤️ Partly cloudy
🌡️ 72°F (22°C)
💧 Humidity: 65%
```
```
