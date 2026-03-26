//! # Ajo Circle Smart Contract (Admin Updated)
//!
//! A decentralized rotating savings and credit association (ROSCA) implementation on Stellar.
//! Updated with Role-Based Access Control (RBAC) for administrative security.

#![no_std]

pub mod factory;

#[cfg(test)]
mod deposit_tests;

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, BytesN, Env, Map, Vec};

/// Default maximum number of members allowed in a circle
const MAX_MEMBERS: u32 = 50;
/// Absolute maximum capacity for any circle
const HARD_CAP: u32 = 100;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum AjoError {
    NotFound = 1,
    Unauthorized = 2,
    AlreadyExists = 3,
    InvalidInput = 4,
    AlreadyPaid = 5,
    InsufficientFunds = 6,
    Disqualified = 7,
    VoteAlreadyActive = 8,
    NoActiveVote = 9,
    AlreadyVoted = 10,
    CircleNotActive = 11,
    CircleAlreadyDissolved = 12,
    CircleAtCapacity = 13,
    CirclePanicked = 14,
    PriceUnavailable = 15,
    ArithmeticOverflow = 16,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CircleData {
    pub organizer: Address,
    pub token_address: Address,
    pub contribution_amount: i128,
    pub frequency_days: u32,
    pub max_rounds: u32,
    pub current_round: u32,
    pub member_count: u32,
    pub max_members: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemberData {
    pub address: Address,
    pub total_contributed: i128,
    pub total_withdrawn: i128,
    pub has_received_payout: bool,
    pub status: u32,
}

#[contracttype]
pub enum DataKey {
    Circle,
    Members,
    Standings,
    Admin, // The security key for Access Control
    KycStatus,
    CircleStatus,
    RotationOrder,
    RoundDeadline,
    RoundContribCount,
    TotalPool,
}

#[contract]
pub struct AjoCircle;

#[contractimpl]
impl AjoCircle {
    /// Internal helper: Ensures the caller has the ADMIN_ROLE.
    /// Replaces simple checks with formal Stellar authentication.
    fn require_admin(env: &Env, admin: &Address) -> Result<(), AjoError> {
        admin.require_auth();

        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(AjoError::NotFound)?;

        if stored_admin != *admin {
            return Err(AjoError::Unauthorized);
        }
        Ok(())
    }

    /// Initialize a new Ajo circle and assign the ADMIN_ROLE to the organizer.
    pub fn initialize_circle(
        env: Env,
        organizer: Address,
        token_address: Address,
        contribution_amount: i128,
        frequency_days: u32,
        max_rounds: u32,
        max_members: u32,
    ) -> Result<(), AjoError> {
        organizer.require_auth();

        let configured_max_members = if max_members == 0 { MAX_MEMBERS } else { max_members };

        if contribution_amount <= 0 || frequency_days == 0 || max_rounds == 0 || configured_max_members > HARD_CAP {
            return Err(AjoError::InvalidInput);
        }

        // Set the Admin Role to the person who started the circle
        env.storage().instance().set(&DataKey::Admin, &organizer);

        let circle_data = CircleData {
            organizer: organizer.clone(),
            token_address,
            contribution_amount,
            frequency_days,
            max_rounds,
            current_round: 1,
            member_count: 1,
            max_members: configured_max_members,
        };

        env.storage().instance().set(&DataKey::Circle, &circle_data);
        // ... (Keep the rest of your storage initialization)
        Ok(())
    }

    /// Update off-chain KYC status. Restricted to ADMIN_ROLE.
    pub fn set_kyc_status(
        env: Env,
        admin: Address,
        member: Address,
        is_verified: bool,
    ) -> Result<(), AjoError> {
        Self::require_admin(&env, &admin)?;

        let mut kyc: Map<Address, bool> = env.storage().instance().get(&DataKey::KycStatus).unwrap_or_else(|| Map::new(&env));
        kyc.set(member, is_verified);
        env.storage().instance().set(&DataKey::KycStatus, &kyc);
        Ok(())
    }

    /// Remove a dormant user. Restricted to ADMIN_ROLE.
    pub fn boot_dormant_member(env: Env, admin: Address, member: Address) -> Result<(), AjoError> {
        Self::require_admin(&env, &admin)?;
        // ... (existing boot logic)
        Ok(())
    }
}
