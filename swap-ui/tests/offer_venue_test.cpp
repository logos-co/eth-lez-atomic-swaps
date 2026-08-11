// Standalone unit test for swap-ui/src/offer_venue.h — the P0 fund-theft guard
// that pins an accepted offer's swap venue (ETH HTLC contract + LEZ HTLC
// program) to the app's canonical values. SwapUiPlugin::offerNamesCanonicalVenue
// and validateConfigForAction("taker") rely on exactly these comparisons.
// Deliberately zero Qt/logos/nix dependencies so it compiles and runs directly:
//
//   c++ -std=c++17 -I../src -o /tmp/offer_venue_test offer_venue_test.cpp \
//     && /tmp/offer_venue_test
//
// Exits 0 and prints "ALL PASSED" on success; non-zero on the first failure.

#include "../src/offer_venue.h"

#include <cstdio>
#include <string>

namespace {

int failures = 0;

void expect(const char* label, bool cond)
{
    if (!cond) {
        std::fprintf(stderr, "FAIL %s\n", label);
        ++failures;
    } else {
        std::printf("ok   %s\n", label);
    }
}

// Canonical pinned values (shape only — the guard is value-agnostic).
const std::string kEth = "0x351B0EA07739FA9F6769213927D7836a790A5FAF";
const std::string kLez = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";

} // namespace

int main()
{
    using swap_ui::checkOfferVenue;
    using swap_ui::hexEquals;

    // hexEquals: case- and 0x-prefix-insensitive.
    expect("eip55 vs lowercase equal", hexEquals(kEth, "0x351b0ea07739fa9f6769213927d7836a790a5faf"));
    expect("0x-prefixed vs bare equal", hexEquals(kLez, "0x" + kLez));
    expect("whitespace trimmed", hexEquals("  " + kEth + "  ", kEth));
    expect("different values not equal", !hexEquals(kEth, "0x0000000000000000000000000000000000000000"));

    // Honest offer: both venue fields match the canonical values → accepted.
    {
        auto r = checkOfferVenue(kEth, kEth, kLez, kLez);
        expect("honest offer accepted", r.ok && r.reason.empty());
    }

    // THE attack: offer names the attacker's OWN ETH HTLC contract → rejected.
    {
        auto r = checkOfferVenue("0xdeadBEEFdeadBEEFdeadBEEFdeadBEEFdeadBEEF", kEth, kLez, kLez);
        expect("non-canonical eth contract rejected", !r.ok);
        expect("eth rejection has a reason", !r.reason.empty());
    }

    // The attack via the LEZ program the maker controls → rejected.
    {
        auto r = checkOfferVenue(
            kEth, kEth,
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff", kLez);
        expect("non-canonical lez program rejected", !r.ok);
    }

    // Case/0x variations of the canonical values still pass (no honest-path
    // regression from formatting differences between maker and taker).
    {
        auto r = checkOfferVenue(
            "351b0ea07739fa9f6769213927d7836a790a5faf", kEth, "0x" + kLez, kLez);
        expect("canonical modulo case/0x accepted", r.ok);
    }

    // An offer that OMITS a venue field is not a bypass: the taker keeps its
    // pinned value, so an empty field is never a mismatch.
    {
        auto r = checkOfferVenue("", kEth, "", kLez);
        expect("omitted venue fields accepted (taker keeps pinned)", r.ok);
    }

    // But a present-and-wrong field alongside an omitted one is still rejected.
    {
        auto r = checkOfferVenue("0x00", kEth, "", kLez);
        expect("present-wrong eth rejected even if lez omitted", !r.ok);
    }

    if (failures > 0) {
        std::fprintf(stderr, "%d assertion(s) FAILED\n", failures);
        return 1;
    }
    std::printf("ALL PASSED\n");
    return 0;
}
