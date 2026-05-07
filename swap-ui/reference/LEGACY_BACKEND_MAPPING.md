# Legacy SwapBackend Mapping

Legacy source snapshot: `6f52949^:logos-module/qml/*` and
`6f52949^:logos-module/src/swap_backend.*`.

The copied QML in `legacy-qml/` is reference material only. Live QML must bind
through `swap-ui/src/swap_ui.rep`, and the C++ UI backend must call the `swap`
core module.

## Surface Inventory

| Old `swapBackend` surface | New owner | Live `swap_ui.rep` mapping |
| --- | --- | --- |
| `ethRpcUrl`, `ethPrivateKey`, `ethHtlcAddress` | UI config state | `ethRpcUrl`, `ethPrivateKey`, `ethHtlcAddress`, `setConfigValue()` |
| `lezSequencerUrl`, `lezSigningKey`, `lezWalletHome`, `lezAccountId`, `lezHtlcProgramId` | UI config state | matching `PROP(QString ...)`, `setConfigValue()` |
| `lezAmount`, `ethAmount`, `lezTimelockMinutes`, `ethTimelockMinutes` | UI config state | matching `PROP(QString ...)`, serialized into core calls |
| `ethRecipientAddress`, `lezTakerAccountId`, `pollIntervalMs`, `wakuBootstrapMultiaddr` | UI config state | matching `PROP(QString ...)`, `setConfigValue()` |
| `swapRole` | UI session state | `swapRole`, `setRole()` |
| `ethAddress`, `ethBalance`, `lezAccount`, `lezBalance` | Derived from core module | `fetchBalances()` calls `swap.fetchBalances(configJson)` and maps returned JSON |
| `makerRunning`, `makerCurrentStep`, `makerProgressSteps`, `makerResultJson` | UI orchestration state | `startMaker()` calls `swap.runMaker(configJson, hashlockHex)` |
| `takerRunning`, `takerCurrentStep`, `takerProgressSteps`, `takerResultJson` | UI orchestration state | `startTaker()` calls `swap.runTaker(configJson, preimageHex)` |
| `autoAcceptRunning`, counters, `swapHistory` | UI orchestration state | `startAutoAccept()` calls `swap.runMakerLoop(configJson)`, `stopAutoAccept()` calls `swap.stopMakerLoop()` |
| `messagingConnected`, `messagingPeerCount` | UI state from core module | `initMessaging()` and polling call `swap.messagingInit()` / `swap.messagingStatus()` |
| `publishOffer()`, `fetchOffers()` plus `offerPublished`, `offersFetched` | Core operation, UI signal adapter | `publishOffer()` / `fetchOffers()` update `offerResultJson` / `offersJson` |
| `refundLez()`, `refundEth()` | Core operation | `refundLez()` / `refundEth()` call `swap.refundLez()` / `swap.refundEth()` |

## Remaining Gaps

Progress events are still not at old parity. The core module emits
`maker.progress`, `taker.progress`, and `maker_loop.progress`, but the UI backend
does not yet subscribe those module events back into `makerProgressSteps`,
`takerProgressSteps`, and `swapHistory`. Until that event bridge is wired, the
ported views show start/finish state and final results, not the old live
step-by-step updates.
