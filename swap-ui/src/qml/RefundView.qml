import QtQuick
import QtQuick.Layouts
import SwapTheme

// The screen a worried user lands on.
//
// It used to open with "Reclaim funds from expired HTLCs. The maker refunds LEZ
// (needs hashlock), the taker refunds ETH (needs swap ID)" and then ask for 64
// characters of hex — the one page in the app where reassurance matters most had
// none at all. It is the same function, reframed as the safety net doing its
// job: what we already know is offered first, and hand-entering an identifier is
// the advanced fallback rather than the entry point.
//
// Honesty note: this view does NOT claim a timelock has expired. It has no
// timelock in hand — only a hashlock and a swap id from the last run — so it
// says what happens if the timer has not passed rather than inventing a
// countdown.
PageScaffold {
    id: refundRoot

    title: "Get your money back"
    subtitle: "If a swap stalls, nothing is lost — locked funds unlock on their "
              + "own once the swap's timer runs out. This page claims them back by hand."

    function isValidHex(s, bytes) {
        let clean = s.startsWith("0x") ? s.substring(2) : s
        if (clean.length !== bytes * 2) return false
        return /^[0-9a-fA-F]+$/.test(clean)
    }

    // Parse last swap results to auto-populate refund fields
    readonly property var lastMakerResult: {
        if (!swapBackend.makerResultJson) return null
        try { return JSON.parse(swapBackend.makerResultJson) }
        catch (e) { return null }
    }
    readonly property var lastTakerResult: {
        if (!swapBackend.takerResultJson) return null
        try { return JSON.parse(swapBackend.takerResultJson) }
        catch (e) { return null }
    }
    readonly property var lastResult: lastMakerResult || lastTakerResult

    // hashlock is available in completed/refunded results, either role
    readonly property string lastHashlock: lastResult ? (lastResult.hashlock || "") : ""

    // `eth_tx` is ROLE-DEPENDENT (src/swap/types.rs): for a taker it is the ETH
    // lock swap id — which is what refundEth() wants — but for a maker it is the
    // ETH *claim transaction hash*, which is a different 32-byte value entirely.
    //
    // Reading it off `lastResult` (maker-first) therefore seeded this field with
    // a claim tx after any maker run. It is 32 bytes, so isValidHex() passes and
    // the button enables on a value that can never resolve to a swap. Only the
    // taker result can supply a swap id. HistoryView.qml:81 already makes this
    // distinction; this view did not.
    readonly property string lastEthSwapId: lastTakerResult
        ? (lastTakerResult.eth_tx || "") : ""

    readonly property bool hasKnownSwap: lastHashlock !== "" || lastEthSwapId !== ""

    // A finished swap has nothing left in the lock, so the chain refuses a refund
    // on it — offering the one-click claim there is offering a guaranteed failure.
    // Only "completed" and "refunded" count as finished: a failure, or a status
    // we cannot read, is exactly the stalled case this page is here for.
    //
    // Checked per SIDE rather than through `lastResult`, which resolves
    // maker-first: someone who sold successfully and then had a purchase stall
    // would otherwise have their still-locked ETH declared finished by the
    // completed sale. Hidden only when nothing is outstanding on either side.
    function swapSettled(result) {
        return result !== null
            && (result.status === "completed" || result.status === "refunded")
    }
    readonly property bool lastSwapFinished:
        (lastMakerResult !== null || lastTakerResult !== null)
        && (lastMakerResult === null || swapSettled(lastMakerResult))
        && (lastTakerResult === null || swapSettled(lastTakerResult))
    readonly property bool busy: swapBackend.running || swapBackend.refundsLoading

    // Manual-entry state. LabeledField is a CONTROLLED field: it re-applies its
    // `text` on every blur, so using `text:` as a one-time seed makes the box
    // revert the instant focus leaves — including the blur from clicking the
    // submit button beside it, which then submits an empty value. So we own the
    // value here and feed it back in.
    //
    // The `touched` flags are what let a user clear a pre-filled box and type a
    // different identifier: without them, an emptied field would fall back to
    // the seed and silently refund the wrong swap.
    property bool ethIdTouched: false
    property string ethIdEntry: ""
    readonly property string ethIdValue: ethIdTouched ? ethIdEntry : lastEthSwapId

    property bool hashlockTouched: false
    property string hashlockEntry: ""
    readonly property string hashlockValue: hashlockTouched ? hashlockEntry : lastHashlock

    SafetyNote {
        text: "A stalled swap cannot lose your funds. Both sides are locked with a "
              + "timer, and when it runs out each side can take its own money back."
    }

    // --- What we already know -------------------------------------------
    Card {
        visible: refundRoot.hasKnownSwap && !refundRoot.lastSwapFinished
        tone: "done"

        RowLayout {
            Layout.fillWidth: true
            spacing: Theme.spacingSmall

            StatusDot { status: "live"; size: 6; Layout.alignment: Qt.AlignVCenter }
            Text {
                text: "Ready to claim back"
                color: Theme.toneLive
                font.pixelSize: Theme.fontCaption
                font.bold: true
                font.letterSpacing: 1
            }
            Item { Layout.fillWidth: true }
        }

        Text {
            Layout.fillWidth: true
            text: "This is from your most recent swap — the details are already filled "
                  + "in below. If the swap's timer has not run out yet, the network will "
                  + "refuse the refund; that is the contract protecting the other side, "
                  + "not an error. Wait and try again."
            color: Theme.textSecondary
            font.pixelSize: Theme.fontSmall
            horizontalAlignment: Text.AlignLeft
            wrapMode: Text.WordWrap
        }

        HexValue {
            visible: refundRoot.lastHashlock !== ""
            label: "Hashlock"
            value: refundRoot.lastHashlock
        }
        HexValue {
            visible: refundRoot.lastEthSwapId !== ""
            label: "Swap ID"
            value: refundRoot.lastEthSwapId
        }

        RowLayout {
            Layout.fillWidth: true
            Layout.topMargin: Theme.spacingSmall
            spacing: Theme.spacingNormal

            PrimaryButton {
                visible: refundRoot.lastEthSwapId !== ""
                text: swapBackend.refundsLoading ? "Claiming…" : "Get my ETH back"
                enabled: !refundRoot.busy
                         && refundRoot.isValidHex(refundRoot.lastEthSwapId, 32)
                Layout.fillWidth: true
                Layout.preferredHeight: 42
                onClicked: swapBackend.refundEth(refundRoot.lastEthSwapId)
            }
            PrimaryButton {
                visible: refundRoot.lastHashlock !== ""
                text: swapBackend.refundsLoading ? "Claiming…" : "Get my LEZ back"
                enabled: !refundRoot.busy
                         && refundRoot.isValidHex(refundRoot.lastHashlock, 32)
                Layout.fillWidth: true
                Layout.preferredHeight: 42
                onClicked: swapBackend.refundLez(refundRoot.lastHashlock)
            }
        }
    }

    // --- Last swap already finished --------------------------------------
    Card {
        visible: refundRoot.hasKnownSwap && refundRoot.lastSwapFinished

        RowLayout {
            Layout.fillWidth: true
            spacing: Theme.spacingSmall

            StatusDot { status: "waiting"; size: 6; Layout.alignment: Qt.AlignVCenter }
            SectionHeader { label: "Nothing to claim back"; hairline: false }
        }

        Text {
            Layout.fillWidth: true
            text: "Your most recent swap finished, so it has nothing left locked up "
                  + "to claim back. For an older swap, use \"Enter a refund by hand "
                  + "(advanced)\" below — its details are on that swap's receipt in "
                  + "History."
            color: Theme.textSecondary
            font.pixelSize: Theme.fontSmall
            horizontalAlignment: Text.AlignLeft
            wrapMode: Text.WordWrap
        }
    }

    // --- Nothing to do ---------------------------------------------------
    Card {
        visible: !refundRoot.hasKnownSwap

        EmptyState {
            Layout.fillWidth: true
            tone: Theme.toneWaiting
            title: "Nothing stuck right now"
            subtitle: "You have no stalled swaps. If one ever stalls, it shows up "
                      + "here with the details already filled in."
        }
    }

    // --- Manual entry ----------------------------------------------------
    Disclosure {
        label: "Enter a refund by hand (advanced)"

        Text {
            Layout.fillWidth: true
            text: "Only needed if you are claiming back a swap this app has not seen "
                  + "— after reinstalling, say, or from another machine. Both identifiers "
                  + "appear on the swap's receipt in History."
            color: Theme.textMuted
            font.pixelSize: Theme.fontCaption
            horizontalAlignment: Text.AlignLeft
            wrapMode: Text.WordWrap
        }

        // --- ETH refund (the taker's side; most users) ---
        Card {
            SectionHeader {
                label: "Get ETH back"
                hairline: false
            }
            Text {
                Layout.fillWidth: true
                text: "For a swap where you locked ETH and the other side never "
                      + "completed. The timer is enforced by the contract on Ethereum."
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
                horizontalAlignment: Text.AlignLeft
                wrapMode: Text.WordWrap
            }
            LabeledField {
                id: ethSwapIdField
                label: "Swap ID"
                placeholderText: "The 64-character ID from your swap receipt"
                text: refundRoot.ethIdValue
                onValueEdited: (val) => {
                    refundRoot.ethIdTouched = true
                    refundRoot.ethIdEntry = val
                }
                maximumLength: 66
                fieldEnabled: !refundRoot.busy
                errorText: refundRoot.ethIdValue.length > 0
                           && !refundRoot.isValidHex(refundRoot.ethIdValue, 32)
                           ? "That does not look like a swap ID — it should be 64 characters of hex."
                           : ""
                hint: "Find it on the swap's receipt in History, labelled \"ETH swap ID\"."
            }
            PrimaryButton {
                text: swapBackend.refundsLoading ? "Claiming…" : "Get my ETH back"
                enabled: !refundRoot.busy
                         && refundRoot.isValidHex(refundRoot.ethIdValue, 32)
                Layout.fillWidth: true
                Layout.preferredHeight: 42
                onClicked: swapBackend.refundEth(refundRoot.ethIdValue)
            }
        }

        // --- LEZ refund (the maker's side) ---
        Card {
            SectionHeader {
                label: "Get LEZ back"
                hairline: false
            }
            Text {
                Layout.fillWidth: true
                text: "For a swap where you locked LEZ in escrow as the seller and the "
                      + "buyer never claimed it."
                color: Theme.textSecondary
                font.pixelSize: Theme.fontSmall
                horizontalAlignment: Text.AlignLeft
                wrapMode: Text.WordWrap
            }
            LabeledField {
                id: lezHashlockField
                label: "Hashlock"
                placeholderText: "The 64-character hashlock from your swap receipt"
                text: refundRoot.hashlockValue
                onValueEdited: (val) => {
                    refundRoot.hashlockTouched = true
                    refundRoot.hashlockEntry = val
                }
                maximumLength: 66
                fieldEnabled: !refundRoot.busy
                errorText: refundRoot.hashlockValue.length > 0
                           && !refundRoot.isValidHex(refundRoot.hashlockValue, 32)
                           ? "That does not look like a hashlock — it should be 64 characters of hex."
                           : ""
                hint: "Find it on the swap's receipt in History, labelled \"Hashlock\"."
            }
            PrimaryButton {
                text: swapBackend.refundsLoading ? "Claiming…" : "Get my LEZ back"
                enabled: !refundRoot.busy
                         && refundRoot.isValidHex(refundRoot.hashlockValue, 32)
                Layout.fillWidth: true
                Layout.preferredHeight: 42
                onClicked: swapBackend.refundLez(refundRoot.hashlockValue)
            }
        }
    }

    // Results
    ResultCard {
        role: "maker"
        resultJson: swapBackend.makerResultJson
    }
    ResultCard {
        role: "taker"
        resultJson: swapBackend.takerResultJson
    }
}
