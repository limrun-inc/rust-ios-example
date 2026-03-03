# Limrun Android & iOS Examples in Rust

This repo provides examples to show how you can use Limrun services
such as iOS & Android simulators as well as XCode sandbox.


## iOS Simulator with Expo Go

The following requests an iOS simulator with Expo Go app pre-installed
and opens the given URL so that once it's returned, Expo Go would be connected
to your Expo dev server URL right away.

```bash
export LIM_API_KEY=...
```

```bash
cargo run -- exp://some-exp-url
```

You should see the following output where you can give `mcpUrl` to your coding
agent to use the iOS Simulator.

```bash
{"instanceId":"ios_euna_01kjtzcc9tf1m8r89bdn5hctkn","openedUrl":"https://www.example.com","mcpUrl":"https://eu-oh1-m2-73b1.limrun.net/v1/ios_euna_01kjtzcc9tf1m8r89bdn5hctkn/mcp"}
```
