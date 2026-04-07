---
name: weather
description: Get weather forecast from Open-Meteo service (free, no API key required)
always: false
bins:
  - curl
  - jq
---

# Weather Forecast Skill

Free weather API from Open-Meteo - no API key required.

## Quick Start

```bash
# Current weather (latitude, longitude)
curl -s "https://api.open-meteo.com/v1/forecast?latitude=23.1291&longitude=113.2644&current=temperature_2m,weather_code,relative_humidity_2m,wind_speed_10m&timezone=auto"

# 3-day forecast
curl -s "https://api.open-meteo.com/v1/forecast?latitude=23.1291&longitude=113.2644&daily=weather_code,temperature_2m_max,temperature_2m_min&timezone=auto&forecast_days=3"
```

## City Coordinates

| City | Lat | Lon |
|------|-----|-----|
| Guangzhou | 23.1291 | 113.2644 |
| Beijing | 39.9042 | 116.4074 |
| Shanghai | 31.2304 | 121.4737 |
| Shenzhen | 22.5431 | 114.0579 |
| Chengdu | 30.5728 | 104.0668 |

## Common Parameters

### Current Weather
- `temperature_2m` - Temperature (°C)
- `weather_code` - Weather condition
- `relative_humidity_2m` - Humidity (%)
- `wind_speed_10m` - Wind speed (km/h)
- `apparent_temperature` - Feels like (°C)
- `precipitation` - Precipitation (mm)

### Daily Forecast
- `temperature_2m_max,temperature_2m_min` - High/Low temps
- `weather_code` - Weather condition
- `precipitation_probability_max` - Rain chance (%)
- `precipitation_sum` - Total precipitation (mm)
- `sunrise,sunset` - Sunrise/sunset times
- `uv_index_max` - Max UV index

### Query Options
```bash
&forecast_days=3          # Days (1-16)
&timezone=auto            # Auto timezone
&temperature_unit=celsius # or fahrenheit
```

## Weather Codes

| Code | Meaning |
|------|---------|
| 0 | Clear sky |
| 1-3 | Partly cloudy |
| 45,48 | Fog |
| 51-55 | Drizzle |
| 61-65 | Rain |
| 71-75 | Snow |
| 80-82 | Rain showers |
| 95-99 | Thunderstorm |

## Examples

### Current Weather (Guangzhou)
```bash
curl -s "https://api.open-meteo.com/v1/forecast?latitude=23.1291&longitude=113.2644&current=temperature_2m,weather_code,relative_humidity_2m,wind_speed_10m,apparent_temperature&timezone=auto" | jq '.current'
```

### 3-Day Forecast
```bash
curl -s "https://api.open-meteo.com/v1/forecast?latitude=23.1291&longitude=113.2644&daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max&timezone=auto&forecast_days=3" | jq '.daily'
```

## Advanced

### Air Quality (Separate API)
```bash
curl -s "https://air-quality-api.open-meteo.com/v1/air-quality?latitude=23.1291&longitude=113.2644&current=pm2_5,pm10,carbon_monoxide,nitrogen_dioxide,sulphur_dioxide,ozone&timezone=auto"
```

### Historical Data
```bash
curl -s "https://api.open-meteo.com/v1/forecast?latitude=23.1291&longitude=113.2644&daily=temperature_2m_max,temperature_2m_min&start_date=2024-01-01&end_date=2024-01-07&timezone=auto"
```

## Notes

- **Rate limit**: 10,000 calls/day
- **Format**: JSON only
- **Docs**: https://open-meteo.com/en/docs
