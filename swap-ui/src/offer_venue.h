#ifndef SWAP_UI_OFFER_VENUE_H
#define SWAP_UI_OFFER_VENUE_H

#include <string>

// Pure, dependency-free venue-pinning checks shared between the plugin
// (swap_ui_plugin.cpp, applyOfferObject / acceptOfferAndStartTaker /
// validateConfigForAction) and its unit test (tests/offer_venue_test.cpp).
// Deliberately excludes Qt/logos headers so the test compiles standalone
// (no nix build / module toolchain needed) and so the exact comparison the
// plugin relies on is exercised directly, not reimplemented in the test.
//
// P0 (fund-theft): an offer names the swap venue — the ETH HTLC contract
// (`eth_htlc_address`) and the LEZ HTLC program (`lez_htlc_program_id`). Those
// are NOT taker-configurable per offer: they must equal the app's canonical,
// pinned deployment. A malicious maker who advertises its OWN contract/program
// could otherwise steer the taker into a venue it controls, learn the preimage
// the taker reveals, and sweep the taker's real ETH. The taker must therefore
// REFUSE any offer that names a non-canonical venue, and NEVER adopt the
// offer's contract/program fields into its config.
namespace swap_ui {

// Normalise a hex value for comparison: strip an optional leading "0x"/"0X",
// drop surrounding ASCII whitespace, and lowercase. Contract addresses and
// program IDs are case-insensitive hex (ETH addresses may be EIP-55
// mixed-case), so a byte-for-byte string compare would produce false
// mismatches; comparing normalised forms is the correct equality.
inline std::string normalizeHex(const std::string& value)
{
    size_t begin = 0;
    size_t end = value.size();
    auto isSpace = [](char c) {
        return c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\f' || c == '\v';
    };
    while (begin < end && isSpace(value[begin])) {
        ++begin;
    }
    while (end > begin && isSpace(value[end - 1])) {
        --end;
    }
    if (end - begin >= 2 && value[begin] == '0'
        && (value[begin + 1] == 'x' || value[begin + 1] == 'X')) {
        begin += 2;
    }
    std::string out;
    out.reserve(end - begin);
    for (size_t i = begin; i < end; ++i) {
        char c = value[i];
        if (c >= 'A' && c <= 'Z') {
            c = static_cast<char>(c - 'A' + 'a');
        }
        out.push_back(c);
    }
    return out;
}

// Case- and 0x-prefix-insensitive hex equality.
inline bool hexEquals(const std::string& a, const std::string& b)
{
    return normalizeHex(a) == normalizeHex(b);
}

// Which venue field (if any) failed the canonical check.
enum class VenueField {
    None,        // matched — no mismatch
    EthContract, // eth_htlc_address differs from canonical
    LezProgram,  // lez_htlc_program_id differs from canonical
};

// Result of checking whether an offer names the canonical, pinned venue.
// On a mismatch, `mismatch` names the offending field and `offered`/`expected`
// carry the raw hex values — for DEBUG LOGGING ONLY. The user-facing message is
// built by the caller (SwapUiPlugin) and deliberately contains no hex: a raw
// 64-char program id or 0x… address in a status line reads as a crash, not the
// "you were just protected" it actually is.
struct VenueCheck {
    bool ok;
    VenueField mismatch;
    std::string offered;
    std::string expected;
};

// Verify that the offer's advertised venue matches the app's canonical values.
// A field that the offer OMITS (empty string) is not treated as a mismatch:
// the taker keeps its pinned value regardless, so an omitted field is only ever
// weaker for an attacker, never a bypass. A field that is PRESENT and differs
// is a hard rejection.
inline VenueCheck checkOfferVenue(const std::string& offerEthHtlc,
                                  const std::string& canonicalEthHtlc,
                                  const std::string& offerLezProgram,
                                  const std::string& canonicalLezProgram)
{
    if (!normalizeHex(offerEthHtlc).empty()
        && !hexEquals(offerEthHtlc, canonicalEthHtlc)) {
        return VenueCheck{false, VenueField::EthContract, offerEthHtlc, canonicalEthHtlc};
    }
    if (!normalizeHex(offerLezProgram).empty()
        && !hexEquals(offerLezProgram, canonicalLezProgram)) {
        return VenueCheck{false, VenueField::LezProgram, offerLezProgram, canonicalLezProgram};
    }
    return VenueCheck{true, VenueField::None, std::string{}, std::string{}};
}

} // namespace swap_ui

#endif // SWAP_UI_OFFER_VENUE_H
