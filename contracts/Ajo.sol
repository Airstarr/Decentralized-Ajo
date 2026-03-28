// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

error InvalidContribution();
error AjoIsFull();
error InitializationFailed();

contract Ajo {
    address public admin;
    uint256 public contributionAmount;
    uint256 public cycleDuration;
    uint32 public maxMembers;
    address[] public members;

    bool private initialized;
    event AjoCreated(address indexed admin, uint256 contributionAmount, uint32 maxMembers);

    function initialize(
        address _admin,
        uint256 _contributionAmount,
        uint256 _cycleDuration,
        uint32 _maxMembers
    ) external {
        if(initialized) revert InitializationFailed();
        
        admin = _admin;
        contributionAmount = _contributionAmount;
        cycleDuration = _cycleDuration;
        maxMembers = _maxMembers;
        initialized = true;

        emit AjoCreated(_admin, _contributionAmount, _maxMembers);
    }
}
