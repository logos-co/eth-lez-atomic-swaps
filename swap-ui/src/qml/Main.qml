import QtQuick
import QtQuick.Controls
import "."

Item {
    id: root

    readonly property var backend: typeof logos !== "undefined" && logos.module
        ? logos.module("swap_ui")
        : null
    property bool backendReady: false
    readonly property bool ready: backend !== null && backendReady

    function watch(reply, onSuccess, onError) {
        if (typeof logos === "undefined" || !logos.watch) {
            if (onError) onError("Logos bridge not available")
            return
        }
        logos.watch(reply,
            function(value) { if (onSuccess) onSuccess(value) },
            function(error) { if (onError) onError(error) }
        )
    }

    Timer {
        interval: 250
        running: true
        repeat: true
        onTriggered: root.backendReady = root.backend !== null
            && typeof logos !== "undefined"
            && logos.isViewModuleReady("swap_ui")
    }

    QtObject {
        id: swapBackend

        readonly property bool ready: root.ready

        readonly property string status: root.backend ? root.backend.status : ""
        readonly property string errorMessage: root.backend ? root.backend.errorMessage : ""
        readonly property string swapRole: root.backend ? root.backend.swapRole : ""
        readonly property string lastResultJson: root.backend ? root.backend.lastResultJson : ""
        readonly property string validationErrorsJson: root.backend ? root.backend.validationErrorsJson : "{}"
        readonly property bool running: root.backend ? root.backend.running : false

        readonly property string ethRpcUrl: root.backend ? root.backend.ethRpcUrl : ""
        readonly property string ethPrivateKey: root.backend ? root.backend.ethPrivateKey : ""
        readonly property string ethHtlcAddress: root.backend ? root.backend.ethHtlcAddress : ""
        readonly property string lezSequencerUrl: root.backend ? root.backend.lezSequencerUrl : ""
        readonly property string lezSigningKey: root.backend ? root.backend.lezSigningKey : ""
        readonly property string lezWalletHome: root.backend ? root.backend.lezWalletHome : ""
        readonly property string lezAccountId: root.backend ? root.backend.lezAccountId : ""
        readonly property string lezHtlcProgramId: root.backend ? root.backend.lezHtlcProgramId : ""
        readonly property string lezAmount: root.backend ? root.backend.lezAmount : ""
        readonly property string ethAmount: root.backend ? root.backend.ethAmount : ""
        readonly property string lezTimelockMinutes: root.backend ? root.backend.lezTimelockMinutes : ""
        readonly property string ethTimelockMinutes: root.backend ? root.backend.ethTimelockMinutes : ""
        readonly property string ethRecipientAddress: root.backend ? root.backend.ethRecipientAddress : ""
        readonly property string lezTakerAccountId: root.backend ? root.backend.lezTakerAccountId : ""
        readonly property string pollIntervalMs: root.backend ? root.backend.pollIntervalMs : ""
        readonly property string wakuBootstrapMultiaddr: root.backend ? root.backend.wakuBootstrapMultiaddr : ""
        readonly property bool balancesLoading: root.backend ? root.backend.balancesLoading : false
        readonly property bool messagingLoading: root.backend ? root.backend.messagingLoading : false
        readonly property bool offersLoading: root.backend ? root.backend.offersLoading : false
        readonly property bool publishingLoading: root.backend ? root.backend.publishingLoading : false
        readonly property bool refundsLoading: root.backend ? root.backend.refundsLoading : false

        readonly property string ethAddress: root.backend ? root.backend.ethAddress : ""
        readonly property string ethBalance: root.backend ? root.backend.ethBalance : ""
        readonly property string lezAccount: root.backend ? root.backend.lezAccount : ""
        readonly property string lezBalance: root.backend ? root.backend.lezBalance : ""

        readonly property bool makerRunning: root.backend ? root.backend.makerRunning : false
        readonly property string makerJobId: root.backend ? root.backend.makerJobId : ""
        readonly property string makerCurrentStep: root.backend ? root.backend.makerCurrentStep : ""
        readonly property var makerProgressSteps: root.backend ? root.backend.makerProgressSteps : []
        readonly property string makerResultJson: root.backend ? root.backend.makerResultJson : ""

        readonly property bool takerRunning: root.backend ? root.backend.takerRunning : false
        readonly property string takerJobId: root.backend ? root.backend.takerJobId : ""
        readonly property string takerCurrentStep: root.backend ? root.backend.takerCurrentStep : ""
        readonly property var takerProgressSteps: root.backend ? root.backend.takerProgressSteps : []
        readonly property string takerResultJson: root.backend ? root.backend.takerResultJson : ""

        readonly property bool autoAcceptRunning: root.backend ? root.backend.autoAcceptRunning : false
        readonly property string autoAcceptJobId: root.backend ? root.backend.autoAcceptJobId : ""
        readonly property int autoAcceptCompleted: root.backend ? root.backend.autoAcceptCompleted : 0
        readonly property int autoAcceptFailed: root.backend ? root.backend.autoAcceptFailed : 0
        readonly property int autoAcceptIteration: root.backend ? root.backend.autoAcceptIteration : 0
        readonly property var swapHistory: root.backend ? root.backend.swapHistory : []

        readonly property bool messagingConnected: root.backend ? root.backend.messagingConnected : false
        readonly property int messagingPeerCount: root.backend ? root.backend.messagingPeerCount : 0
        readonly property string offersJson: root.backend ? root.backend.offersJson : ""
        readonly property string offerResultJson: root.backend ? root.backend.offerResultJson : ""

        signal offersFetched(string offersJson)
        signal offerPublished(string resultJson)

        function setConfigValue(key, value) {
            if (!root.ready) return
            root.watch(root.backend.setConfigValue(key, value))
        }

        function setRole(role) {
            if (!root.ready) return
            root.watch(root.backend.setRole(role))
        }

        function loadEnvFile(path, role) {
            if (!root.ready) return
            root.watch(root.backend.loadEnvFile(path, role))
        }

        function validateConfig() {
            if (!root.ready) return false
            root.watch(root.backend.validateConfig())
            return true
        }

        function fetchBalances() {
            if (!root.ready) return
            root.watch(root.backend.fetchBalances())
        }

        function startMaker(hashlockHex) {
            if (!root.ready) return
            root.watch(root.backend.startMaker(hashlockHex || ""))
        }

        function startTaker(preimageHex) {
            if (!root.ready) return
            root.watch(root.backend.startTaker(preimageHex || ""))
        }

        function acceptOfferAndStartTaker(offer) {
            if (!root.ready || !offer) return
            root.watch(root.backend.acceptOfferAndStartTaker(JSON.stringify(offer)))
        }

        function refundLez(hashlockHex) {
            if (!root.ready) return
            root.watch(root.backend.refundLez(hashlockHex))
        }

        function refundEth(swapIdHex) {
            if (!root.ready) return
            root.watch(root.backend.refundEth(swapIdHex))
        }

        function publishOffer() {
            if (!root.ready) return
            root.watch(root.backend.publishOffer())
        }

        function fetchOffers() {
            if (!root.ready) return
            root.watch(root.backend.fetchOffers())
        }

        function startAutoAccept() {
            if (!root.ready) return
            root.watch(root.backend.startAutoAccept())
        }

        function stopAutoAccept() {
            if (!root.ready) return
            root.watch(root.backend.stopAutoAccept())
        }
    }

    Connections {
        target: root.backend
        ignoreUnknownSignals: true

        function onOffersJsonChanged() {
            if (swapBackend.offersJson !== "")
                swapBackend.offersFetched(swapBackend.offersJson)
        }

        function onOfferResultJsonChanged() {
            if (swapBackend.offerResultJson !== "")
                swapBackend.offerPublished(swapBackend.offerResultJson)
        }
    }

    Rectangle {
        anchors.fill: parent
        color: Theme.background
    }

    AtomicSwapView {
        anchors.fill: parent
    }
}
