// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Script, console} from "forge-std/Script.sol";
import {EthHTLC} from "../src/EthHTLC.sol";

contract DeployEthHTLC is Script {
    function run() public {
        uint256 deployerPrivateKey = vm.envUint("PRIVATE_KEY");

        vm.startBroadcast(deployerPrivateKey);

        uint256 minTimelockDelta = vm.envOr("MIN_TIMELOCK_DELTA", uint256(300));
        EthHTLC htlc = new EthHTLC(minTimelockDelta);

        vm.stopBroadcast();

        console.log("EthHTLC deployed at:", address(htlc));
    }
}
