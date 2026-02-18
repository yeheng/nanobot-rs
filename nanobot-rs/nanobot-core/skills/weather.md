---
name: weather
description: Weather information lookup using web APIs
always: false
bins:
  - curl
---

# Weather Skill

This skill provides weather information lookup capabilities using various weather APIs.

## Supported Services

### wttr.in (Default)

Simple weather service that works without API keys.

```bash
# Current weather
curl wttr.in/?format=3

# Full weather report
curl wttr.in/

# Specific location
curl wttr.in/Beijing

# Short format
curl wttr.in/?format="%t+%c"

# JSON format for parsing
curl wttr.in/?format=j1
```

### Weather API Integration

For more detailed weather data, you can use:

- OpenWeatherMap API
- WeatherAPI.com
- AccuWeather API

## Common Weather Queries

### Current Conditions

```bash
# Get current temperature and conditions
curl "wttr.in/?format=%t+%c&lang=en"

# Get detailed current weather
curl "wttr.in/?format=j1" | jq .current_condition[0]
```

### Forecast

```bash
# 3-day forecast
curl wttr.in/

# Today's forecast only
curl "wttr.in/?format=j1" | jq .weather[0]
```

### Location-based

```bash
# Weather by city
curl wttr.in/Shanghai

# Weather by coordinates (approximate)
curl wttr.in/~31.23,121.47

# Weather by airport code
curl wttr.in/~PEK
```

## Output Formats

| Format Specifier | Description |
|-----------------|-------------|
| `%t` | Temperature |
| `%c` | Weather condition |
| `%h` | Humidity |
| `%w` | Wind speed |
| `%p` | Precipitation |

## Usage Examples

When asked about weather:

1. "What's the weather in Beijing?" → `curl wttr.in/Beijing?format=3`
2. "Will it rain tomorrow?" → Check forecast with `curl wttr.in/?format=j1`
3. "What's the temperature?" → `curl wttr.in/?format=%t`

## Notes

- wttr.in has rate limits, use responsibly
- For production use, consider registering for a proper weather API key
- Location detection is automatic based on IP if no location specified
