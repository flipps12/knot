# Knot ingress

## JSON Socket (12012)

```
{ command: "command", AdditionalOptions: "...", ... }
```

```
{ response: "response" }
```

### Commands: 
- status
- connect
    - addr: address
- connectrelay
    - relay_addr: address
    - relay_id: peerid
- discover
    - peer_id: peerid
- newappname
    - name: custom name
    - port: port of socket client (number)

### Response
- status
    - OK
- newappname
    - app_id
