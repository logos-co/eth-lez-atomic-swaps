pragma Singleton
import QtQuick

// Block-explorer URL builder for the receipt's trust surface. Every URL is
// derived from an on-chain fact carried on the wire (a chain id, an
// already-observed tx hash/address) — never from a config field. A config
// key would be a claim from a file; a chain id is a fact from the chain,
// so unknown/absent chains simply resolve to "" (today's copy-only
// behaviour) instead of linking to the wrong network.
QtObject {
    id: links

    readonly property string lezExplorerBase: "https://explorer.testnet.lez.logos.co"
    // Substring match against network.lez_sequencer — tolerant of a
    // trailing slash / scheme variance in how the sequencer URL was
    // configured, while still refusing to link a non-testnet deployment.
    readonly property string lezTestnetHostPattern: "testnet.lez.logos.co"

    // Anvil's chain ids (31337 default, 1337 also seen) intentionally fall
    // through to "": a local devnet has no public explorer.
    function ethBase(chainId) {
        var id = Number(chainId || 0)
        if (id === 1) return "https://etherscan.io"
        if (id === 11155111) return "https://sepolia.etherscan.io"
        return ""
    }

    function ethTx(hash, chainId) {
        var base = links.ethBase(chainId)
        if (base === "" || !hash) return ""
        return base + "/tx/" + hash
    }

    function ethAddress(addr, chainId) {
        var base = links.ethBase(chainId)
        if (base === "" || !addr) return ""
        return base + "/address/" + addr
    }

    // Verified live: /transaction/<hash> (not /tx/<hash>, which 404s)
    // resolves with a bare, lowercase, 0x-stripped 64-hex hash.
    function lezTx(hash) {
        if (!hash) return ""
        var bare = String(hash).replace(/^0x/i, "").toLowerCase()
        return links.lezExplorerBase + "/transaction/" + bare
    }

    function lezAccount(id) {
        if (!id) return ""
        return links.lezExplorerBase + "/account/" + id
    }

    // Gate for lezTx/lezAccount: the LEZ explorer only indexes the public
    // testnet, so a receipt from any other sequencer (a local devnet, a
    // future mainnet) must not offer a link that resolves nowhere useful.
    function isLezTestnet(sequencerUrl) {
        return typeof sequencerUrl === "string"
            && sequencerUrl.indexOf(links.lezTestnetHostPattern) >= 0
    }
}
