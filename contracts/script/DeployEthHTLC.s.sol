// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Script, console} from "forge-std/Script.sol";
import {EthHTLC} from "../src/EthHTLC.sol";

contract DeployEthHTLC is Script {
    /// @dev ABI marker for this revision of EthHTLC. `lock` now takes the
    ///      taker's LEZ AccountId, so a deployment of an older build is NOT
    ///      interchangeable with this one — clients must be repointed at the
    ///      address logged below.
    string constant LOCK_SIGNATURE = "lock(bytes32,uint256,address,bytes32)";

    function run() public {
        uint256 deployerPrivateKey = vm.envUint("PRIVATE_KEY");

        vm.startBroadcast(deployerPrivateKey);

        uint256 minTimelockDelta = vm.envOr("MIN_TIMELOCK_DELTA", uint256(300));
        EthHTLC htlc = new EthHTLC(minTimelockDelta);

        vm.stopBroadcast();

        console.log("EthHTLC deployed at:", address(htlc));
        console.log("  INTERFACE_VERSION:", htlc.INTERFACE_VERSION());
        console.log("  minTimelockDelta: ", minTimelockDelta);
        console.log("  lock signature:   ", LOCK_SIGNATURE);
        console.log("Set eth_htlc_address to the address above in every offer/config.");
    }
}
