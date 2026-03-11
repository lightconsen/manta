# Weather Skill

A skill to fetch weather information for any location.

## Triggers

- Regex: `weather (?:in|for)?\s*(.+)`
- Keyword: "weather"
- Intent: "get_weather"

## Prompt

When the user asks about weather, fetch current weather data for the specified location.

Use the `web_search` tool to find current weather information if you don't have recent data.

Respond with:
- Current temperature
- Weather conditions (sunny, cloudy, rain, etc.)
- Humidity
- Wind speed
- Any weather alerts

## Example Usage

**User:** "What's the weather in New York?"

**Action:** Fetch weather data for New York City

**Response:**
```
Current Weather in New York:
🌤️ Partly cloudy
🌡️ Temperature: 72°F (22°C)
💧 Humidity: 65%
💨 Wind: 8 mph NW
```

## Configuration

Optionally set a default location:

```yaml
default_location: "San Francisco, CA"
units: "imperial"  # or "metric"
```

## Notes

- This skill uses web search to find weather data
- For production use, consider using a weather API like OpenWeatherMap
- Cache results for 15 minutes to reduce API calls
