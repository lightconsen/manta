---
name: weather
description: "Get weather information for locations"
version: "1.0.0"
author: "manta"
triggers:
  - type: command
    pattern: "weather"
    priority: 100
  - type: keyword
    pattern: "weather"
    priority: 90
  - type: keyword
    pattern: "forecast"
    priority: 80
  - type: keyword
    pattern: "temperature"
    priority: 80
openclaw:
  emoji: "🌤️"
  category: "productivity"
  tags:
    - "weather"
    - "forecast"
    - "location"
  requires:
    bins: ["curl"]
---

# Weather Skill

Get current weather conditions and forecasts for any location.

## Data Sources

Uses free weather APIs (no API key required):
- wttr.in - Command-line weather service
- Open-Meteo - Free weather API

## Capabilities

### Current Weather
- Temperature (current, feels like)
- Conditions (sunny, cloudy, rain, etc.)
- Humidity and wind
- Visibility and pressure

### Forecasts
- Hourly forecast (next 24 hours)
- Daily forecast (next 7 days)
- Precipitation probability

### Location Support
- City name (e.g., "London", "New York")
- ZIP/postal code
- Airport code (e.g., "JFK", "LHR")
- Lat/Lon coordinates
- Auto-detect by IP

## Usage Examples

### Current weather
```bash
# By city
curl -s wttr.in/London?format=3

# Full report
curl -s wttr.in/London

# Short format
curl -s wttr.in/London?format="%l:+%c+%t"
```

### Forecast
```bash
# 3-day forecast
curl -s wttr.in/London?format=v2
```

## Output Formatting

wttr.in supports various format codes:
- `%l` - Location
- `%c` - Weather condition
- `%t` - Temperature
- `%h` - Humidity
- `%w` - Wind
- `%p` - Precipitation

## Best Practices

1. Use format=3 for concise output
2. Handle location not found errors
3. Cache results for 15 minutes to reduce API calls
4. Fallback to alternative APIs if one fails
