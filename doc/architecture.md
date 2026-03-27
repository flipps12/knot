In spanish for now

# Arquitectura

Un Thread recibe cada paquete en forma de JSON, se parse para empaquetar y determinar nodo de salida. Es enviado por MPSC al Core Engine que consultara la tabla de peers y determinara si se debe buscar via DHT, crear tunel via Hole Punching o simplemente enviar los datos, Dependiendo del estado de la conexion.
Por ultimo otro Thread toma el mensaje, lo fragmenta si es necesario y se envia por QUIC o UDP, segun las necesidades del mensaje.

Worker A -> Core -> Worker B

Un main lanzara 3 Tokio Tasks que recibiran como parametros los sender de las demas instancias para comunicaion lineal.

El primer Thread se encargara de el control de sockets a nivel sistema, contara con un socket unico para crear nuevos sockets especificos por aplicacion segmentando la comunicacion por practicidad. Cada dato entrante sera reenviado por un sender al Core especificando su procedencia, su destino y payload.

El Core recibiendo paquetes consulatara el hashmap con todos los peers comunicados recientemente, se determiara si es necesario hole punching, peer finding via DHT o simplemente enviar los bytes via QUIC/UDP. Todos estos casos terminan en un comando enviado por sender a el ultimo Thread.

El tercer Thread tendra la instancia de LibP2P y contara con funciones especificas por comando recibido, cumpliendo con la decision que se tome en el Core, ultimo lugar de empaquetado de Bytes.

Worker A <- Core <- Worker B

Definir