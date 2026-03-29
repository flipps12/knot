## Funcionamiento basico

El Main socekt TCP es 127.0.0.1:12012 se encarga de iniciar los demas sockets. Los demas sockets estaran conectados a un socket de retorno para la comunicacion bidireccional. Todos los sockets tendran esta sintaxis: 

```
{"value": "127.0.0.1:1234", "command": "newchannel"}
```

El comando newchannel creara un socket con el parametro is_main = false, lo que permite registrar el socket de retorno.

```
{"value": "message", "command": "forward_to_central"}
```

El comando forward_to_central envia un mensaje hasta el task principal y es reenviado en direccion contraria, desde el cliente por el socket de retorno se recibe el value si todo esta funcionando.