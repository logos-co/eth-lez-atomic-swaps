// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Script, console} from "forge-std/Script.sol";
import {EthHTLC} from "../src/EthHTLC.sol";

contract DeployEthHTLC is Script {
    function run() public {
        uint256 deployerPrivateKey = vm.envUint("PRIVATE_KEY");

        vm.startBroadcast(deployerPrivateKey);

        EthHTLC htlc = new EthHTLC();

        vm.stopBroadcast();

        console.log("EthHTLC deployed at:", address(htlc));
    }
}
