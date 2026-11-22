# picto-relay-session

The host's side of a Picto Relay session, with no I/O.

The ESP32 does everything on the radio. Whatever drives it over USB has the
same things to work out, and they are here:

- what goes in a JOIN and in the DRAW commands that upload a drawing;
- who is in the room, their profiles, and the drawings they send, from the
  frames the host DS rebroadcasts (`Room`);
- converting between the API's bitmaps and the DS's native drawings (`bitmap`).

Reading and writing the serial port, and all timing, are the caller's. There
are two: [picto-relay-server](../picto-relay-server/README.md), and the web
client's demo page through [picto-relay-wasm](../picto-relay-wasm/README.md).
