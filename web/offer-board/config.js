// Runtime configuration for the offer board (not bundled — editable on the
// deployed site without a rebuild).
//
// BOOTSTRAP: the logos.dev delivery fleet currently exposes only raw TCP
// (port 30303), which browsers cannot dial. Until the fleet operators expose
// a WSS (secure websocket) listener, list any WSS-capable cluster-2 peer(s)
// here as full multiaddrs, e.g.:
//   "/dns4/delivery-01.do-ams3.logos.dev.status.im/tcp/443/wss/p2p/16Uiu2HAmTUbnxLGT9JvV6mu9oPyDjqHK4Phs1VDJNUgESgNSkuby"
//
// With an empty list the board loads but shows a "blocked" status explaining
// exactly what is missing.
window.OFFER_BOARD_CONFIG = {
  // WSS multiaddrs of cluster-2 fleet peers reachable from browsers.
  bootstrapPeers: [],
  // Maker heartbeat interval (seconds) — offers older than 2x this + 30s are dropped.
  heartbeatSecs: 45,
};
