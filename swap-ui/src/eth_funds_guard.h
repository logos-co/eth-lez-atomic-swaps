// Pre-flight ETH funds guard + insufficient-funds error detection.
//
// Why this exists (fix/insufficient-eth-guard): a taker with 0 ETH could
// click Buy on the Market board, watch "Generate your secret" succeed, and
// then have step 2 "Lock your ETH" die with the raw JSON-RPC text
// ("error code -32000: ... insufficient funds for gas * price + value ...")
// splashed across the screen twice. The two halves of the fix live here:
//
//   1. insufficientEthForOffer() — decide BEFORE starting a taker swap
//      whether the known ETH balance can cover the offer plus a rough gas
//      allowance. Exact decimal-string arithmetic, because wei amounts
//      (1 ETH = 1e18 wei) overflow int64 for balances above ~9.2 ETH and
//      silently lose precision in doubles.
//   2. isInsufficientEthError() — recognise an insufficient-funds failure
//      that still slipped through (balance changed mid-swap, race), so the
//      UI can headline plain language instead of raw JSON-RPC text.
//
// Deliberately has zero Qt/logos/nix dependencies so it compiles in the
// plain-C++ swap-ui unit harness (`make swap-ui-unit`):
//
//   c++ -std=c++17 -Iswap-ui/src swap-ui/tests/eth_funds_guard_test.cpp \
//     && ./a.out
//
// The QML side of the same guard (OfferBoard.qml's hasEnoughEth) keeps a
// copy of kEthGasHeadroomWei; tests/insufficient-eth-guard.test.mjs asserts
// the two constants never drift apart.

#pragma once

#include <algorithm>
#include <cctype>
#include <string>

namespace swap_ui {

// Rough gas allowance added on top of the offer's wei amount: 0.0005 ETH.
// The app has no live gas-price feed, so this is a fixed margin sized to
// cover an HTLC lock (plus the later claim) at typical Sepolia base fees.
// It is deliberately modest — its job is to catch "can't even pay for gas",
// not to predict fees precisely.
inline constexpr const char* kEthGasHeadroomWei = "500000000000000";

// True iff `s` is a non-empty string of ASCII digits — the only shape a
// known wei balance/amount takes. Anything else means "unknown".
inline bool isDecimalWei(const std::string& s)
{
    if (s.empty()) {
        return false;
    }
    return std::all_of(s.begin(), s.end(), [](unsigned char c) {
        return std::isdigit(c) != 0;
    });
}

namespace detail {

inline std::string stripLeadingZeros(const std::string& s)
{
    const auto firstNonZero = s.find_first_not_of('0');
    if (firstNonZero == std::string::npos) {
        return "0";
    }
    return s.substr(firstNonZero);
}

// Exact decimal-string comparison: negative if a < b, 0 if equal, positive
// if a > b. Both inputs must satisfy isDecimalWei().
inline int compareDecimal(const std::string& a, const std::string& b)
{
    const auto na = stripLeadingZeros(a);
    const auto nb = stripLeadingZeros(b);
    if (na.size() != nb.size()) {
        return na.size() < nb.size() ? -1 : 1;
    }
    return na.compare(nb);
}

// Exact decimal-string addition. Both inputs must satisfy isDecimalWei().
inline std::string addDecimal(const std::string& a, const std::string& b)
{
    std::string result;
    result.reserve(std::max(a.size(), b.size()) + 1);
    int carry = 0;
    auto ia = a.rbegin();
    auto ib = b.rbegin();
    while (ia != a.rend() || ib != b.rend() || carry != 0) {
        int digit = carry;
        if (ia != a.rend()) {
            digit += *ia++ - '0';
        }
        if (ib != b.rend()) {
            digit += *ib++ - '0';
        }
        carry = digit / 10;
        result.push_back(static_cast<char>('0' + digit % 10));
    }
    std::reverse(result.begin(), result.end());
    return result;
}

} // namespace detail

// True only when balanceWei is a KNOWN wei amount that cannot cover
// offerWei + headroomWei.
//
// An unknown balance ("" — never fetched, or fetch failed — or any
// non-decimal shape) never blocks: the guard's job is to stop a swap that
// is certain to fail, not to lock the Buy button behind a flaky balance
// read. A known "0", on the other hand, must block. An unparseable offer
// amount also never blocks — the guard cannot judge what it cannot read.
inline bool insufficientEthForOffer(const std::string& balanceWei,
                                    const std::string& offerWei,
                                    const std::string& headroomWei = kEthGasHeadroomWei)
{
    if (!isDecimalWei(balanceWei) || !isDecimalWei(offerWei)
        || !isDecimalWei(headroomWei)) {
        return false;
    }
    const auto needed = detail::addDecimal(offerWei, headroomWei);
    return detail::compareDecimal(balanceWei, needed) < 0;
}

// Does raw backend/RPC error text describe an insufficient-ETH failure?
// Matches the substring every Ethereum node/client shape of this failure
// carries ("insufficient funds for gas * price + value", "insufficient
// funds", ...), case-insensitively.
inline bool isInsufficientEthError(const std::string& raw)
{
    static const std::string needle = "insufficient funds";
    if (raw.size() < needle.size()) {
        return false;
    }
    std::string lowered(raw);
    std::transform(lowered.begin(), lowered.end(), lowered.begin(),
                   [](unsigned char c) {
                       return static_cast<char>(std::tolower(c));
                   });
    return lowered.find(needle) != std::string::npos;
}

// The plain-language headline shown when an insufficient-funds error still
// reaches the UI mid-swap. The QML side (SwapCopy/Copy.qml, friendlyError)
// carries the same sentence for the receipt card;
// tests/insufficient-eth-guard.test.mjs asserts the two never drift apart.
inline constexpr const char* kInsufficientEthDisplayCopy =
    "Your ETH balance is too low to pay for this swap (amount + gas). "
    "Add Sepolia test ETH from the Setup tab and try again.";

} // namespace swap_ui
