---
name: weather
description: Weather forecast via Open-Meteo (free, no API key)
always: false
bins:
  - curl
  - jq
---

# Weather

Open-Meteo API: free, no key, 10k calls/day.

## Quick Query

```bash
curl -s "https://api.open-meteo.com/v1/forecast?latitude=23.1291&longitude=113.2644&current=temperature_2m,weather_code,relative_humidity_2m,wind_speed_10m&timezone=auto"
```

## Parameters

### Current
- `temperature_2m`, `weather_code`, `relative_humidity_2m`, `wind_speed_10m`, `apparent_temperature`, `precipitation`

### Daily
- `temperature_2m_max,temperature_2m_min`, `weather_code`, `precipitation_probability_max`, `precipitation_sum`, `sunrise,sunset`, `uv_index_max`

### Options
- `forecast_days=1-16`, `timezone=auto`, `temperature_unit=celsius|fahrenheit`

## Weather Codes

| Code | Meaning |
|------|---------|
| 0 | Clear |
| 1-3 | Partly cloudy |
| 45,48 | Fog |
| 51-55 | Drizzle |
| 61-65 | Rain |
| 71-75 | Snow |
| 80-82 | Showers |
| 95-99 | Thunderstorm |

## Air Quality

```bash
curl -s "https://air-quality-api.open-meteo.com/v1/air-quality?latitude=23.1291&longitude=113.2644&current=pm2_5,pm10&timezone=auto"
```

Docs: https://open-meteo.com/en/docs
