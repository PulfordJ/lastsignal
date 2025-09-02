# LastSignal

LastSignal is an automated safety check-in system built in Rust. It monitors your well-being by requiring periodic check-ins and automatically notifies emergency contacts if you fail to check in within a specified timeframe.

## Features

- **Automated Check-in Reminders**: Sends reminders via multiple channels (email, Facebook Messenger) to prompt you to check in
- **Emergency Contact Notification**: Automatically sends a "last signal" to configured emergency contacts if you don't check in
- **WHOOP Integration**: Automatic check-ins via WHOOP fitness tracker activity (requires WHOOP account and OAuth setup)
- **Multiple Output Channels**: Supports email and Facebook Messenger (in progress - see limitations below) with health checks and automatic failover
- **Persistent State Tracking**: Keeps track of check-ins, requests, and system state across restarts
- **Configurable Timing**: Fully customizable intervals for check-ins and emergency notifications
- **Health Monitoring**: Tests all configured outputs and falls back to alternatives if primary methods fail
- **Simple CLI Interface**: Easy commands for running the daemon, manual check-ins, and system status

## How It Works

1. **Check-in Phase**: After a configured number of days without a check-in, LastSignal sends you reminders via your preferred channels
2. **Last Signal Phase**: If you still don't check in after additional time, the system sends a detailed emergency message to your configured emergency contacts
3. **Automatic Failover**: If any communication channel fails its health check, the system immediately tries the next configured option
4. **Persistent Memory**: The system remembers its state across restarts and system reboots

## Installation

### Prerequisites

- Rust (latest stable version)
- Optional: Email SMTP server access (e.g., Gmail with app passwords) - needed for email check-ins or messaging recipients
- Optional: WHOOP account for automatic check-ins via fitness tracker activity

### Build from Source

```bash
git clone https://github.com/yourusername/lastsignal.git
cd lastsignal
cargo build --release
```

The binary will be available at `target/release/lastsignal`.

### Install

```bash
# Install to /usr/local/bin (or your preferred location)
sudo cp target/release/lastsignal /usr/local/bin/

# Or install via cargo
cargo install --path .
```

## Configuration

### Setup Configuration Directory

Create the configuration directory and copy the example config:

```bash
mkdir -p ~/.lastsignal/
cp examples/config.toml ~/.lastsignal/config.toml
cp examples/last_signal_message.txt ~/.lastsignal/last_signal_message.txt
```

### Duration Format

All time-based configuration values use a duration format with explicit units:

- **Seconds**: `"30s"`, `"45sec"`, `"60seconds"`  
- **Minutes**: `"5m"`, `"10min"`, `"30minutes"`
- **Hours**: `"2h"`, `"6hr"`, `"24hours"`  
- **Days**: `"1d"`, `"7day"`, `"30days"`

Examples:
- `"7d"` = 7 days
- `"24h"` = 24 hours  
- `"30m"` = 30 minutes
- `"3600s"` = 3600 seconds (1 hour)

**Important**: Pure numbers like `7` or `3600` are not supported - you must include a unit.

### Edit Configuration

Edit `~/.lastsignal/config.toml` to match your needs:

```toml
[checkin]
duration_between_checkins = "7d"  # Ask for check-in every 7 days
output_retry_delay = "24h"

[[checkin.outputs]]
type = "email"
config = { 
    to = "your-email@example.com", 
    smtp_host = "smtp.gmail.com", 
    smtp_port = "587", 
    username = "your-email@gmail.com", 
    password = "your-app-password" 
}

[recipient]
max_time_since_last_checkin = "14d"  # Send emergency message after 14 days since last successful check-in

[[recipient.last_signal_outputs]]
type = "email"
config = { 
    to = "emergency-contact@example.com", 
    smtp_host = "smtp.gmail.com", 
    smtp_port = "587", 
    username = "your-email@gmail.com", 
    password = "your-app-password" 
}
```

### Configure Email (Gmail Example)

1. Enable 2-factor authentication on your Gmail account
2. Generate an App Password: Google Account → Security → 2-Step Verification → App Passwords
3. Use the app password in your configuration

### Configure WHOOP Integration (Optional)

WHOOP integration allows automatic check-ins based on your fitness tracker activity:

1. You'll need a WHOOP account and active subscription
2. Run `lastsignal setup-whoop` to authenticate via OAuth
3. Configure automatic check-in thresholds based on your activity preferences
4. WHOOP tokens are automatically refreshed in the background

See [TODO](TODO) for other potential future integrations including Facebook Messenger.

## Usage

### Run the Daemon

Start LastSignal to continuously monitor and send notifications:

```bash
lastsignal run
```

This will run indefinitely, checking every hour whether notifications need to be sent.

### Manual Check-in

Record a manual check-in to reset the timer:

```bash
lastsignal checkin
```

### Check Status

View current system status and configuration:

```bash
lastsignal status
```

### Test Outputs

Test all configured communication channels:

```bash
lastsignal test
```

### Running as a Service

#### systemd (Linux)

Create `/etc/systemd/system/lastsignal.service`:

```ini
[Unit]
Description=LastSignal Safety Check-in System
After=network.target

[Service]
Type=simple
User=yourusername
ExecStart=/usr/local/bin/lastsignal run
Restart=always
RestartSec=30

[Install]
WantedBy=multi-user.target
```

Enable and start:

```bash
sudo systemctl enable lastsignal
sudo systemctl start lastsignal
```

#### macOS (launchd)

Create `~/Library/LaunchAgents/com.yourusername.lastsignal.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.yourusername.lastsignal</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/lastsignal</string>
        <string>run</string>
    </array>
    <key>KeepAlive</key>
    <true/>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
```

Load and start:

```bash
launchctl load ~/Library/LaunchAgents/com.yourusername.lastsignal.plist
launchctl start com.yourusername.lastsignal
```

## Configuration Reference

### Checkin Section

- `duration_between_checkins`: Duration to wait between check-in requests
- `output_retry_delay`: Duration to wait between output attempts (ignored if health checks fail)
- `outputs`: Array of output configurations for check-in reminders

### Recipient Section

- `max_time_since_last_checkin`: Maximum time since last successful check-in before sending emergency message
- `output_retry_delay`: Duration to wait between emergency notification attempts
- `last_signal_outputs`: Array of output configurations for emergency contacts

### Output Types

#### Email

```toml
[[checkin.outputs]]
type = "email"
config = { 
    to = "recipient@example.com",
    from = "sender@example.com",  # Optional, defaults to username
    smtp_host = "smtp.gmail.com",
    smtp_port = "587",
    username = "sender@example.com",
    password = "app_password"
}
```

#### Facebook Messenger

**Status: In Progress - Partially Stalled**

The Facebook Messenger adapter implementation is currently in progress but faces technical limitations. Facebook's webhook system requires a publicly accessible HTTPS endpoint, which cannot be localhost. This means additional infrastructure (reverse proxy, tunneling service, or cloud deployment) is required to fully integrate Facebook Messenger notifications.

```toml
[[checkin.outputs]]
type = "facebook_messenger"
config = { 
    user_id = "facebook_user_id",
    access_token = "page_access_token"
}
```

**Current Limitations:**
- Webhook verification requires a publicly accessible HTTPS URL
- Development/testing requires ngrok, cloudflare tunnel, or similar service
- Production deployment needs proper webhook infrastructure

### Last Signal Configuration

- `adapter_type`: Currently only "file" is supported
- `message_file`: Path to the message template file

### App Configuration

- `data_directory`: Directory for state and log files (default: `~/.lastsignal/`)
- `log_level`: Logging verbosity (trace, debug, info, warn, error)

## State Management

LastSignal maintains state in `~/.lastsignal/state.json`:

- `last_checkin`: Timestamp of last successful check-in
- `last_checkin_request`: Timestamp of last check-in request sent
- `last_signal_fired`: Timestamp of last emergency signal sent
- `checkin_request_count`: Number of check-in requests sent

## Security Considerations

- Store sensitive credentials (passwords, tokens) securely
- Consider using environment variables for sensitive configuration
- Regularly rotate access tokens and passwords
- Use app-specific passwords for email services
- Ensure the configuration file has appropriate permissions (`chmod 600 ~/.lastsignal/config.toml`)

## Troubleshooting

### Check Logs

```bash
# Run with debug logging
RUST_LOG=debug lastsignal run

# Or set log level in config
[app]
log_level = "debug"
```

### Test Outputs

```bash
lastsignal test
```

### Common Issues

1. **Email authentication errors**: Ensure you're using app passwords, not your main account password
2. **Facebook Messenger errors**: Facebook Messenger integration is currently in development and requires additional webhook infrastructure. See the Facebook Messenger section above for current limitations
3. **Permission errors**: Ensure the user running LastSignal can write to the data directory

## Contributing

Contributions are welcome! Please feel free to submit pull requests or open issues for bugs and feature requests.

## License

This project is licensed under the MIT License - see the LICENSE file for details.

## Disclaimer

LastSignal is designed as a safety tool, but should not be relied upon as your only safety measure. Always maintain multiple emergency contacts and safety protocols. The developers are not responsible for any issues arising from the use of this software.