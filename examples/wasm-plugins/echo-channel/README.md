# Echo Channel - WASM Plugin Example

This is a simple example WASM plugin for Syscity that demonstrates the channel extension interface. It echoes back any messages it receives.

## Building

```bash
cd examples/wasm-plugins/echo-channel
cargo build --release --target wasm32-wasi
```

## Installation

```bash
# Copy the built WASM file to Syscity's extensions directory
cp target/wasm32-wasi/release/echo-channel.wasm ~/.syscity/extensions/channels/

# Copy the manifest
cp echo-channel.yaml ~/.syscity/extensions/channels/
```

## Usage

```bash
# Load and start the plugin
syscity plugin load echo
syscity plugin start echo

# Or use the extended registry which auto-discovers
syscity server
```

## Configuration

The echo channel accepts the following configuration (JSON):

```json
{
  "prefix": "Echo",
  "include_timestamp": true
}
```

## Architecture

This plugin implements the `channel-plugin` WIT interface defined in `wit/channel.wit`:

- `init()` - Initialize with configuration
- `start()` - Begin listening for messages
- `stop()` - Stop the channel
- `send()` - Send a message (echoes it back)
- `get_capabilities()` - Advertise supported features
- `health_check()` - Report health status

## Testing

Once loaded, you can test by sending a message through the channel:

```bash
# The echo channel will receive the message and echo it back
syscity channel send echo "Hello World"
```
