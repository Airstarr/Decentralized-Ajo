//! # Ajo Circle Smart Contract
//!
//! A decentralized rotating savings and credit association (ROSCA) implementation on Stellar.
//! This contract enables groups to pool funds periodically, with members taking turns receiving payouts.
//!
//! ## Core Features
//! - Member management with configurable limits
//! - Periodic contribution tracking
//! - Rotating payout system with shuffled order
//! - Governance via dissolution voting
//! - Emergency panic mechanism for fund recovery
//! - KYC status tracking
//! - Token-agnostic design (supports any Stellar token)

#![no_std]

pub mod factory;

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, BytesN, Env, Map, Vec};

/// Default maximum number of members allowed in a circle
const MAX_MEMBERS: u32 = 50;
/// Absolute maximum capacity for any circle
const HARD_CAP: u32 = 100;

/// Error codes returned by contract operations
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum AjoError {
    /// Requested resource does not exist
    NotFound = 1,
    /// Caller lacks permission for this operation
    Unauthorized = 2,
    /// Resource already exists (e.g., duplicate member)
    AlreadyExists = 3,
    /// Invalid parameter provided
    InvalidInput = 4,
    /// Member has already received their payout for this round
    AlreadyPaid = 5,
    /// Insufficient balance for withdrawal
    InsufficientFunds = 6,
    /// Member is disqualified due to missed contributions
    Disqualified = 7,
    /// A dissolution vote is already in progress
    VoteAlreadyActive = 8,
    /// No active dissolution vote exists
    NoActiveVote = 9,
    /// Member has already cast their vote
    AlreadyVoted = 10,
    /// Circle is not in the required state for this operation
    CircleNotActive = 11,
    /// Circle has already been dissolved
    CircleAlreadyDissolved = 12,
    /// Circle has reached maximum member capacity
    CircleAtCapacity = 13,
    /// Circle is in emergency panic state
    CirclePanicked = 14,
    /// Oracle price data is unavailable
    PriceUnavailable = 15,
    /// Arithmetic operation would overflow
    ArithmeticOverflow = 16,
}

/// Core circle configuration and state
///
/// Stores the fundamental parameters and current state of an Ajo circle.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CircleData {
    /// Address of the circle organizer (admin)
    pub organizer: Address,
    /// Token contract address (e.g., USDC, XLM)
    pub token_address: Address,
    /// Required contribution amount per round
    pub contribution_amount: i128,
    /// Duration of each round in days
    pub frequency_days: u32,
    /// Total number of rounds in the circle lifecycle
    pub max_rounds: u32,
    /// Current active round number (1-indexed)
    pub current_round: u32,
    /// Current number of active members
    pub member_count: u32,
    /// Maximum allowed members
    pub max_members: u32,
}

/// Individual member data and contribution history
///
/// Tracks each member's financial activity within the circle.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemberData {
    /// Member's wallet address
    pub address: Address,
    /// Cumulative amount contributed to the circle
    pub total_contributed: i128,
    /// Cumulative amount withdrawn from the circle
    pub total_withdrawn: i128,
    /// Whether member has received their scheduled payout
    pub has_received_payout: bool,
    /// Member status: 0 = Active, 1 = Inactive, 2 = Exited
    pub status: u32,
}

/// Circle lifecycle status
///
/// Represents the current operational state of the circle.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CircleStatus {
    /// Normal operation - contributions and payouts active
    Active,
    /// Dissolution vote in progress
    VotingForDissolution,
    /// Circle dissolved via governance vote
    Dissolved,
    /// Emergency state - only refunds allowed
    Panicked,
}

/// Tracks an in-progress dissolution vote
///
/// Stores voting state during the dissolution process.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DissolutionVote {
    /// Number of votes in favor of dissolution
    pub votes_for: u32,
    /// Total number of eligible voting members
    pub total_members: u32,
    /// Threshold mode: 0 = simple majority (>50%), 1 = supermajority (>66%)
    pub threshold_mode: u32,
}

/// Member standing and activity tracking
///
/// Monitors member participation and eligibility status.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemberStanding {
    /// Number of consecutive missed contribution rounds
    pub missed_count: u32,
    /// Whether member is currently active (not disqualified)
    pub is_active: bool,
}

/// Storage keys for contract data
///
/// Defines all persistent storage locations used by the contract.
#[contracttype]
pub enum DataKey {
    /// Core circle configuration (CircleData)
    Circle,
    /// Map of all members (Map<Address, MemberData>)
    Members,
    /// Member activity standings (Map<Address, MemberStanding>)
    Standings,
    /// Circle administrator address
    Admin,
    /// KYC verification status per member (Map<Address, bool>)
    KycStatus,
    /// Current circle lifecycle status (CircleStatus)
    CircleStatus,
    /// Active dissolution vote data (DissolutionVote)
    DissolutionVote,
    /// Tracks which members have voted (Map<Address, bool>)
    VoteCast,
    /// Shuffled payout rotation order (Vec<Address>)
    RotationOrder,
    /// Round deadline timestamp (ledger seconds)
    RoundDeadline,
    /// Number of members who completed current round contribution
    RoundContribCount,
    /// ETH/USD oracle price (scaled by EthUsdDecimals)
    EthUsdPrice,
    /// Decimals used by EthUsdPrice
    EthUsdDecimals,
    /// Last successful deposit timestamp per member (Map<Address, u64>)
    LastDepositAt,
    /// Running total of tokens received via deposit (on-chain accounting)
    TotalPool,
    /// Tracks withdrawals per cycle: Map<cycle_number, Map<member_address, withdrawn>>
    CycleWithdrawals,
}

/// Main Ajo Circle contract
///
/// Implements a decentralized rotating savings and credit association (ROSCA).
/// Members contribute periodically and receive payouts in a predetermined order.
#[contract]
pub struct AjoCircle;

#[contractimpl]
impl AjoCircle {
    /// Verify that the caller is the circle administrator
    ///
    /// # Arguments
    /// * `env` - Contract environment
    /// * `admin` - Address claiming admin privileges
    ///
    /// # Returns
    /// * `Ok(())` if authorized
    /// * `Err(AjoError::Unauthorized)` if not the admin
    /// * `Err(AjoError::NotFound)` if admin not set
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

    /// Calculate 10^exp with overflow checking
    ///
    /// # Arguments
    /// * `exp` - Exponent value
    ///
    /// # Returns
    /// * `Ok(i128)` - Result of 10^exp
    /// * `Err(AjoError::ArithmeticOverflow)` if overflow occurs
    fn pow10_checked(exp: u32) -> Result<i128, AjoError> {
        let mut result: i128 = 1;
        let mut i: u32 = 0;
        while i < exp {
            result = result
                .checked_mul(10)
                .ok_or(AjoError::ArithmeticOverflow)?;
            i += 1;
        }
        Ok(result)
    }

    /// Initialize a new Ajo circle
    ///
    /// Creates a new savings circle with specified parameters. The organizer
    /// becomes the first member and administrator.
    ///
    /// # Arguments
    /// * `env` - Contract environment
    /// * `organizer` - Address of the circle creator (becomes admin)
    /// * `token_address` - Address of the token contract to use (e.g., USDC)
    /// * `contribution_amount` - Required contribution per round
    /// * `frequency_days` - Duration of each round in days
    /// * `max_rounds` - Total number of rounds in the circle
    /// * `max_members` - Maximum number of members (0 = use default)
    ///
    /// # Returns
    /// * `Ok(())` on success
    /// * `Err(AjoError::InvalidInput)` if parameters are invalid
    ///
    /// # Requirements
    /// - Caller must be the organizer
    /// - All numeric parameters must be positive
    /// - max_members must not exceed HARD_CAP
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

        let configured_max_members = if max_members == 0 {
            MAX_MEMBERS
        } else {
            max_members
        };

        if contribution_amount <= 0
            || frequency_days == 0
            || max_rounds == 0
            || configured_max_members == 0
            || configured_max_members > HARD_CAP
        {
            return Err(AjoError::InvalidInput);
        }

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
        env.storage().instance().set(&DataKey::Admin, &organizer);
        env.storage().instance().set(&DataKey::RoundContribCount, &0_u32);

        // Set first round deadline: now + frequency_days converted to seconds
        let deadline = env.ledger().timestamp() + (frequency_days as u64) * 86_400;
        env.storage().instance().set(&DataKey::RoundDeadline, &deadline);

        let mut members: Map<Address, MemberData> = Map::new(&env);
        members.set(
            organizer.clone(),
            MemberData {
                address: organizer.clone(),
                total_contributed: 0,
                total_withdrawn: 0,
                has_received_payout: false,
                status: 0,
            },
        );

        env.storage().instance().set(&DataKey::Members, &members);

        let mut standings: Map<Address, MemberStanding> = Map::new(&env);
        standings.set(
            organizer.clone(),
            MemberStanding {
                missed_count: 0,
                is_active: true,
            },
        );
        env.storage().instance().set(&DataKey::Standings, &standings);

        Ok(())
    }

    /// Join an existing circle as a new member
    ///
    /// Adds a new member to the circle. Only the organizer can add members.
    ///
    /// # Arguments
    /// * `env` - Contract environment
    /// * `organizer` - Address of the circle organizer
    /// * `new_member` - Address of the member to add
    ///
    /// # Returns
    /// * `Ok(())` on success
    /// * `Err(AjoError::Unauthorized)` if caller is not the organizer
    /// * `Err(AjoError::AlreadyExists)` if member already in circle
    /// * `Err(AjoError::CircleAtCapacity)` if circle is full
    /// * `Err(AjoError::CirclePanicked)` if circle is in emergency state
    ///
    /// # Requirements
    /// - Caller must be the organizer
    /// - Member must not already exist
    /// - Circle must not be at capacity
    /// - Circle must not be in panic state
    pub fn join_circle(env: Env, organizer: Address, new_member: Address) -> Result<(), AjoError> {
        organizer.require_auth();

        // Block joins during panic
        if Self::get_circle_status(env.clone()) == CircleStatus::Panicked {
            return Err(AjoError::CirclePanicked);
        }

        let mut circle: CircleData = env
            .storage()
            .instance()
            .get(&DataKey::Circle)
            .ok_or(AjoError::NotFound)?;

        if circle.organizer != organizer {
            return Err(AjoError::Unauthorized);
        }

        let mut members: Map<Address, MemberData> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .ok_or(AjoError::NotFound)?;

        if members.contains_key(new_member.clone()) {
            return Err(AjoError::AlreadyExists);
        }

        if circle.member_count >= circle.max_members {
            return Err(AjoError::CircleAtCapacity);
        }

        members.set(
            new_member.clone(),
            MemberData {
                address: new_member.clone(),
                total_contributed: 0,
                total_withdrawn: 0,
                has_received_payout: false,
                status: 0,
            },
        );

        circle.member_count = circle
            .member_count
            .checked_add(1)
            .ok_or(AjoError::InvalidInput)?;

        let mut standings: Map<Address, MemberStanding> = env.storage()
            .instance()
            .get(&DataKey::Standings)
            .unwrap_or(Map::new(&env));
        
        standings.set(
            new_member.clone(),
            MemberStanding {
                missed_count: 0,
                is_active: true,
            },
        );

        env.storage().instance().set(&DataKey::Members, &members);
        env.storage().instance().set(&DataKey::Circle, &circle);
        env.storage().instance().set(&DataKey::Standings, &standings);

        Ok(())
    }

    /// Backward-compatible wrapper for joining the circle
    ///
    /// Alias for `join_circle` to maintain API compatibility.
    ///
    /// # Arguments
    /// * `env` - Contract environment
    /// * `organizer` - Address of the circle organizer
    /// * `new_member` - Address of the member to add
    ///
    /// # Returns
    /// Same as `join_circle`
    pub fn add_member(env: Env, organizer: Address, new_member: Address) -> Result<(), AjoError> {
        Self::join_circle(env, organizer, new_member)
    }

    /// Record a contribution from a member
    ///
    /// Allows a member to contribute tokens to the circle. Transfers tokens
    /// from the member to the contract and updates their contribution balance.
    ///
    /// # Arguments
    /// * `env` - Contract environment
    /// * `member` - Address of the contributing member
    /// * `amount` - Amount of tokens to contribute
    ///
    /// # Returns
    /// * `Ok(())` on success
    /// * `Err(AjoError::InvalidInput)` if amount <= 0
    /// * `Err(AjoError::NotFound)` if member not in circle
    /// * `Err(AjoError::Disqualified)` if member is inactive
    /// * `Err(AjoError::CirclePanicked)` if circle is in emergency state
    ///
    /// # Requirements
    /// - Caller must be the member
    /// - Amount must be positive
    /// - Member must be active (not disqualified)
    /// - Circle must not be in panic state
    ///
    /// # Side Effects
    /// - Resets member's missed contribution count
    /// - May advance to next round if all members have contributed
    pub fn contribute(env: Env, member: Address, amount: i128) -> Result<(), AjoError> {
        member.require_auth();

        // Block contributions during panic
        if Self::get_circle_status(env.clone()) == CircleStatus::Panicked {
            return Err(AjoError::CirclePanicked);
        }

        if amount <= 0 {
            return Err(AjoError::InvalidInput);
        }

        let mut circle: CircleData = env
            .storage()
            .instance()
            .get(&DataKey::Circle)
            .ok_or(AjoError::NotFound)?;

        let mut standings: Map<Address, MemberStanding> = env.storage()
            .instance()
            .get(&DataKey::Standings)
            .unwrap_or(Map::new(&env));

        if let Some(mut standing) = standings.get(member.clone()) {
            if standing.missed_count >= 3 {
                panic!("Member disqualified due to inactivity.");
            }
            if !standing.is_active {
                return Err(AjoError::Disqualified);
            }
            // Reset missed count on successful contribution
            standing.missed_count = 0;
            standings.set(member.clone(), standing);
        } else {
            return Err(AjoError::NotFound);
        }

        env.storage().instance().set(&DataKey::Standings, &standings);

        let mut members: Map<Address, MemberData> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .ok_or(AjoError::NotFound)?;

        if let Some(mut member_data) = members.get(member.clone()) {
            let round_target = (circle.current_round as i128)
                .checked_mul(circle.contribution_amount)
                .ok_or(AjoError::ArithmeticOverflow)?;
            let had_completed_round = member_data.total_contributed >= round_target;

            // Transfer tokens from member to contract
            let token_client = token::Client::new(&env, &circle.token_address);
            token_client.transfer(&member, &env.current_contract_address(), &amount);

            member_data.total_contributed = member_data
                .total_contributed
                .checked_add(amount)
                .ok_or(AjoError::ArithmeticOverflow)?;

            let has_completed_round = member_data.total_contributed >= round_target;

            members.set(member.clone(), member_data);

            if !had_completed_round && has_completed_round {
                let mut round_contrib_count: u32 = env
                    .storage()
                    .instance()
                    .get(&DataKey::RoundContribCount)
                    .unwrap_or(0_u32);

                round_contrib_count = round_contrib_count
                    .checked_add(1)
                    .ok_or(AjoError::ArithmeticOverflow)?;

                if round_contrib_count >= circle.member_count {
                    let deadline: u64 = env
                        .storage()
                        .instance()
                        .get(&DataKey::RoundDeadline)
                        .unwrap_or(0);
                    let next_deadline = deadline + (circle.frequency_days as u64) * 86_400;
                    env.storage().instance().set(&DataKey::RoundDeadline, &next_deadline);

                    if circle.current_round < circle.max_rounds {
                        circle.current_round += 1;
                    }

                    round_contrib_count = 0;
                    env.storage().instance().set(&DataKey::Circle, &circle);
                }

                env.storage()
                    .instance()
                    .set(&DataKey::RoundContribCount, &round_contrib_count);
            }
        } else {
            return Err(AjoError::NotFound);
        }

        env.storage().instance().set(&DataKey::Members, &members);

        Ok(())
    }

    /// Deposit exactly the configured periodic contribution amount in the circle token.
    /// Records the ledger timestamp for the member and increments the tracked pool balance.
    pub fn deposit(env: Env, member: Address) -> Result<(), AjoError> {
        member.require_auth();

        if Self::get_circle_status(env.clone()) == CircleStatus::Panicked {
            return Err(AjoError::CirclePanicked);
        }

        let circle: CircleData = env
            .storage()
            .instance()
            .get(&DataKey::Circle)
            .ok_or(AjoError::NotFound)?;

        let amount = circle.contribution_amount;
        if amount <= 0 {
            return Err(AjoError::InvalidInput);
        }

        let mut standings: Map<Address, MemberStanding> = env
            .storage()
            .instance()
            .get(&DataKey::Standings)
            .unwrap_or(Map::new(&env));

        if let Some(mut standing) = standings.get(member.clone()) {
            if standing.missed_count >= 3 {
                panic!("Member disqualified due to inactivity.");
            }
            if !standing.is_active {
                return Err(AjoError::Disqualified);
            }
            standing.missed_count = 0;
            standings.set(member.clone(), standing);
        } else {
            return Err(AjoError::NotFound);
        }

        env.storage().instance().set(&DataKey::Standings, &standings);

        let mut members: Map<Address, MemberData> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .ok_or(AjoError::NotFound)?;

        if let Some(mut member_data) = members.get(member.clone()) {
            let token_client = token::Client::new(&env, &circle.token_address);
            token_client.transfer(&member, &env.current_contract_address(), &amount);

            member_data.total_contributed += amount;
            members.set(member.clone(), member_data);
        } else {
            return Err(AjoError::NotFound);
        }

        let ts = env.ledger().timestamp();
        let mut last_deposits: Map<Address, u64> = env
            .storage()
            .instance()
            .get(&DataKey::LastDepositAt)
            .unwrap_or_else(|| Map::new(&env));
        last_deposits.set(member.clone(), ts);
        env.storage().instance().set(&DataKey::LastDepositAt, &last_deposits);

        let mut pool: i128 = env.storage().instance().get(&DataKey::TotalPool).unwrap_or(0);
        pool = pool.checked_add(amount).ok_or(AjoError::InvalidInput)?;
        env.storage().instance().set(&DataKey::TotalPool, &pool);

        env.storage().instance().set(&DataKey::Members, &members);

        let round_contributions = members
            .iter()
            .filter(|(_, m)| {
                m.total_contributed >= (circle.current_round as i128) * circle.contribution_amount
            })
            .count() as u32;

        if round_contributions >= circle.member_count {
            let deadline: u64 = env
                .storage()
                .instance()
                .get(&DataKey::RoundDeadline)
                .unwrap_or(0);
            let next_deadline = deadline + (circle.frequency_days as u64) * 86_400;
            env.storage()
                .instance()
                .set(&DataKey::RoundDeadline, &next_deadline);
        }

        // Emit DepositReceived event
        env.events().publish(
            (symbol_short!("deposit"), member.clone()),
            (amount, circle.current_round)
        );

        Ok(())
    }

    /// Running total of tokens received through `deposit` (tracked in instance storage).
    pub fn get_total_pool(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::TotalPool)
            .unwrap_or(0)
    }

    /// Last `deposit` timestamp for a member, if any.
    pub fn get_last_deposit_timestamp(env: Env, member: Address) -> Result<u64, AjoError> {
        let m: Map<Address, u64> = env
            .storage()
            .instance()
            .get(&DataKey::LastDepositAt)
            .ok_or(AjoError::NotFound)?;
        m.get(member).ok_or(AjoError::NotFound)
    }

    /// Shuffle the payout rotation order using ledger sequence as seed (Fisher-Yates).
    /// Must be called by the organizer before the first round begins.
    pub fn shuffle_rotation(env: Env, organizer: Address) -> Result<(), AjoError> {
        organizer.require_auth();

        let circle: CircleData = env.storage()
            .instance()
            .get(&DataKey::Circle)
            .ok_or(AjoError::NotFound)?;

        if circle.organizer != organizer {
            return Err(AjoError::Unauthorized);
        }

        // Block shuffle during panic
        if Self::get_circle_status(env.clone()) == CircleStatus::Panicked {
            return Err(AjoError::CirclePanicked);
        }

        let members: Map<Address, MemberData> = env.storage()
            .instance()
            .get(&DataKey::Members)
            .ok_or(AjoError::NotFound)?;

        // Build ordered list from current members
        let mut rotation: Vec<Address> = Vec::new(&env);
        for (addr, _) in members.iter() {
            rotation.push_back(addr);
        }

        let n = rotation.len();
        if n < 2 {
            env.storage().instance().set(&DataKey::RotationOrder, &rotation);
            return Ok(());
        }

        // Seed: mix ledger sequence with tx hash bytes for unpredictability
        let ledger_seq = env.ledger().sequence();
        let tx_hash: BytesN<32> = env.crypto().sha256(
            &soroban_sdk::Bytes::from_slice(&env, &ledger_seq.to_be_bytes())
        ).into();
        let hash_bytes = tx_hash.to_array();

        // Fisher-Yates shuffle — seed advances through hash bytes cyclically
        for i in (1..n).rev() {
            let byte_idx = (i as usize) % 32;
            let j = (hash_bytes[byte_idx] as u32) % (i + 1);
            // Swap rotation[i] and rotation[j]
            let a = rotation.get(i).unwrap();
            let b = rotation.get(j).unwrap();
            rotation.set(i, b);
            rotation.set(j, a);
        }

        env.storage().instance().set(&DataKey::RotationOrder, &rotation);

        Ok(())
    }

    /// Slash a member for missing a contribution round
    pub fn slash_member(env: Env, admin: Address, member: Address) -> Result<(), AjoError> {
        Self::require_admin(&env, &admin)?;

        let mut standings: Map<Address, MemberStanding> = env.storage()
            .instance()
            .get(&DataKey::Standings)
            .unwrap_or(Map::new(&env));

        if let Some(mut standing) = standings.get(member.clone()) {
            standing.missed_count += 1;
            if standing.missed_count >= 3 {
                standing.is_active = false;
            }
            standings.set(member.clone(), standing);
            env.storage().instance().set(&DataKey::Standings, &standings);
            Ok(())
        } else {
            Err(AjoError::NotFound)
        }
    }

    /// Update off-chain KYC tie for a member. Admin-only.
    pub fn set_kyc_status(
        env: Env,
        admin: Address,
        member: Address,
        is_verified: bool,
    ) -> Result<(), AjoError> {
        Self::require_admin(&env, &admin)?;

        let mut kyc: Map<Address, bool> = env
            .storage()
            .instance()
            .get(&DataKey::KycStatus)
            .unwrap_or_else(|| Map::new(&env));

        kyc.set(member, is_verified);
        env.storage().instance().set(&DataKey::KycStatus, &kyc);

        Ok(())
    }

    /// Remove a dormant user from active standing. Admin-only.
    pub fn boot_dormant_member(
        env: Env,
        admin: Address,
        member: Address,
    ) -> Result<(), AjoError> {
        Self::require_admin(&env, &admin)?;

        let mut standings: Map<Address, MemberStanding> = env
            .storage()
            .instance()
            .get(&DataKey::Standings)
            .unwrap_or(Map::new(&env));

        if let Some(mut standing) = standings.get(member.clone()) {
            standing.is_active = false;
            standings.set(member.clone(), standing);
        } else {
            return Err(AjoError::NotFound);
        }

        let mut members: Map<Address, MemberData> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .ok_or(AjoError::NotFound)?;

        if let Some(mut member_data) = members.get(member.clone()) {
            member_data.status = 2;
            members.set(member, member_data);
        } else {
            return Err(AjoError::NotFound);
        }

        env.storage().instance().set(&DataKey::Standings, &standings);
        env.storage().instance().set(&DataKey::Members, &members);

        Ok(())
    }

    /// Upgrade the contract's WASM code. Restricted to admin.
    pub fn upgrade(env: Env, admin: Address, new_wasm_hash: BytesN<32>) -> Result<(), AjoError> {
        Self::require_admin(&env, &admin)?;
        env.deployer().update_current_contract_wasm(new_wasm_hash);
        Ok(())
    }

    /// Claim payout when it's a member's turn
    /// REENTRANCY PROTECTED: Follows Checks-Effects-Interactions pattern
    pub fn claim_payout(env: Env, member: Address) -> Result<i128, AjoError> {
        member.require_auth();

        // CHECKS: Validate all conditions first
        // Block payouts during panic
        if Self::get_circle_status(env.clone()) == CircleStatus::Panicked {
            return Err(AjoError::CirclePanicked);
        }

        let circle: CircleData = env
            .storage()
            .instance()
            .get(&DataKey::Circle)
            .ok_or(AjoError::NotFound)?;

        // Validate cycle is within valid range
        if cycle == 0 || cycle > circle.max_rounds {
            return Err(AjoError::InvalidInput);
        }

        // Check member standing
        let standings: Map<Address, MemberStanding> = env.storage()
            .instance()
            .get(&DataKey::Standings)
            .unwrap_or(Map::new(&env));

        if let Some(standing) = standings.get(member.clone()) {
            if !standing.is_active {
                return Err(AjoError::Disqualified);
            }
        }

        // Verify cycle has matured (time check)
        let current_time = env.ledger().timestamp();
        let cycle_deadline = Self::get_cycle_deadline(&env, cycle)?;
        
        if current_time < cycle_deadline {
            return Err(AjoError::InvalidInput); // Cycle not yet mature
        }

        // Verify pool is fully funded for this cycle
        let required_pool = (circle.member_count as i128) * circle.contribution_amount;
        if !Self::is_cycle_fully_funded(&env, cycle, required_pool)? {
            return Err(AjoError::InsufficientFunds);
        }

        // Enforce rotation order - verify member is designated recipient for this cycle
        if let Some(rotation) = env.storage()
            .instance()
            .get::<DataKey, Vec<Address>>(&DataKey::RotationOrder)
        {
            let idx = (cycle - 1) as u32;
            let expected = rotation.get(idx).ok_or(AjoError::InvalidInput)?;
            if expected != member {
                return Err(AjoError::Unauthorized);
            }
        } else {
            return Err(AjoError::InvalidInput); // Rotation not set
        }

        // Check if already withdrawn for this cycle
        let mut cycle_withdrawals: Map<u32, Map<Address, bool>> = env
            .storage()
            .instance()
            .get(&DataKey::CycleWithdrawals)
            .unwrap_or(Map::new(&env));

        let mut cycle_map = cycle_withdrawals
            .get(cycle)
            .unwrap_or(Map::new(&env));

        if cycle_map.get(member.clone()).unwrap_or(false) {
            return Err(AjoError::AlreadyPaid);
        }

            // EFFECTS: Update state BEFORE external call
            member_data.has_received_payout = true;
            member_data.total_withdrawn += payout;

            members.set(member.clone(), member_data);
            env.storage().instance().set(&DataKey::Members, &members);
        } else {
            return Err(AjoError::NotFound);
        }

        // Safe transfer: Execute AFTER state updates (reentrancy protection)
        let token_client = token::Client::new(&env, &circle.token_address);
        token_client.transfer(&env.current_contract_address(), &member, &payout);

        Ok(payout)
    }

    /// Helper: Calculate deadline for a specific cycle
    fn get_cycle_deadline(env: &Env, cycle: u32) -> Result<u64, AjoError> {
        let circle: CircleData = env
            .storage()
            .instance()
            .get(&DataKey::Circle)
            .ok_or(AjoError::NotFound)?;

        let initial_deadline: u64 = env
            .storage()
            .instance()
            .get(&DataKey::RoundDeadline)
            .unwrap_or(0);

            // INTERACTIONS: External call happens LAST
            let token_client = token::Client::new(&env, &circle.token_address);
            token_client.transfer(&env.current_contract_address(), &member, &payout);

            // Emit FundsWithdrawn event
            env.events().publish(
                (symbol_short!("withdraw"), member.clone()),
                (payout, circle.current_round)
            );

            Ok(payout)
        } else {
            0
        };

        let deadline = initial_deadline + (cycles_elapsed as u64) * (circle.frequency_days as u64) * 86_400;
        Ok(deadline)
    }

    /// Helper: Check if cycle is fully funded
    fn is_cycle_fully_funded(env: &Env, cycle: u32, required_amount: i128) -> Result<bool, AjoError> {
        let members: Map<Address, MemberData> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .ok_or(AjoError::NotFound)?;

        let circle: CircleData = env
            .storage()
            .instance()
            .get(&DataKey::Circle)
            .ok_or(AjoError::NotFound)?;

        let target_per_member = (cycle as i128) * circle.contribution_amount;
        
        let mut funded_count = 0_u32;
        for (_, member_data) in members.iter() {
            if member_data.total_contributed >= target_per_member {
                funded_count += 1;
            }
        }

        Ok(funded_count >= circle.member_count)
    }

    /// Legacy claim_payout function - now wraps withdraw() for backward compatibility
    pub fn claim_payout(env: Env, member: Address) -> Result<i128, AjoError> {
        let circle: CircleData = env
            .storage()
            .instance()
            .get(&DataKey::Circle)
            .ok_or(AjoError::NotFound)?;

        // Use current round as the cycle
        Self::withdraw(env, member, circle.current_round)
    }

    /// Perform a partial withdrawal with penalty
    /// REENTRANCY PROTECTED: Follows Checks-Effects-Interactions pattern
    pub fn partial_withdraw(env: Env, member: Address, amount: i128) -> Result<i128, AjoError> {
        member.require_auth();

        // CHECKS: Validate all conditions first
        // Block partial withdrawals during panic — use emergency_refund instead
        if Self::get_circle_status(env.clone()) == CircleStatus::Panicked {
            return Err(AjoError::CirclePanicked);
        }

        if amount <= 0 {
            return Err(AjoError::InvalidInput);
        }

        let circle: CircleData = env
            .storage()
            .instance()
            .get(&DataKey::Circle)
            .ok_or(AjoError::NotFound)?;

        let mut members: Map<Address, MemberData> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .ok_or(AjoError::NotFound)?;

        if let Some(mut member_data) = members.get(member.clone()) {
            let available = member_data.total_contributed - member_data.total_withdrawn;

            if amount > available {
                return Err(AjoError::InsufficientFunds);
            }

            let net_amount = amount - (amount * 10) / 100;

            // EFFECTS: Update state BEFORE external call
            member_data.total_withdrawn += amount;

            members.set(member.clone(), member_data);
            env.storage().instance().set(&DataKey::Members, &members);

            // INTERACTIONS: External call happens LAST
            let token_client = token::Client::new(&env, &circle.token_address);
            token_client.transfer(&env.current_contract_address(), &member, &net_amount);

            Ok(net_amount)
        } else {
            Err(AjoError::NotFound)
        }
    }

    /// Get circle state
    pub fn get_circle_state(env: Env) -> Result<CircleData, AjoError> {
        env.storage()
            .instance()
            .get(&DataKey::Circle)
            .ok_or(AjoError::NotFound)
    }

    /// Get member balance and status
    pub fn get_member_balance(env: Env, member: Address) -> Result<MemberData, AjoError> {
        let members: Map<Address, MemberData> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .ok_or(AjoError::NotFound)?;

        members.get(member).ok_or(AjoError::NotFound)
    }

    /// Get all members
    pub fn get_members(env: Env) -> Result<Vec<MemberData>, AjoError> {
        let members: Map<Address, MemberData> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .ok_or(AjoError::NotFound)?;

        let mut members_vec = Vec::new(&env);
        for (_, member) in members.iter() {
            members_vec.push_back(member);
        }

        Ok(members_vec)
    }

    // ─── Dissolution Voting ───────────────────────────────────────────────────

    /// Start a dissolution vote. Any active member or the organizer may call this.
    /// `threshold_mode`: 0 = simple majority (>50%), 1 = supermajority (>66%).
    pub fn start_dissolution_vote(
        env: Env,
        caller: Address,
        threshold_mode: u32,
    ) -> Result<(), AjoError> {
        caller.require_auth();

        if threshold_mode > 1 {
            return Err(AjoError::InvalidInput);
        }

        // Circle must exist and be active
        let circle: CircleData = env
            .storage()
            .instance()
            .get(&DataKey::Circle)
            .ok_or(AjoError::NotFound)?;

        let status: CircleStatus = env
            .storage()
            .instance()
            .get(&DataKey::CircleStatus)
            .unwrap_or(CircleStatus::Active);

        match status {
            CircleStatus::Dissolved => return Err(AjoError::CircleAlreadyDissolved),
            CircleStatus::VotingForDissolution => return Err(AjoError::VoteAlreadyActive),
            CircleStatus::Panicked => return Err(AjoError::CirclePanicked),
            CircleStatus::Active => {}
        }

        // Caller must be a member or the organizer
        let members: Map<Address, MemberData> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .ok_or(AjoError::NotFound)?;

        if !members.contains_key(caller.clone()) && circle.organizer != caller {
            return Err(AjoError::Unauthorized);
        }

        let vote = DissolutionVote {
            votes_for: 0,
            total_members: circle.member_count,
            threshold_mode,
        };

        env.storage()
            .instance()
            .set(&DataKey::CircleStatus, &CircleStatus::VotingForDissolution);
        env.storage()
            .instance()
            .set(&DataKey::DissolutionVote, &vote);
        env.storage()
            .instance()
            .set(&DataKey::VoteCast, &Map::<Address, bool>::new(&env));

        Ok(())
    }

    /// Cast a YES vote for dissolution. Each member may vote once.
    /// If the threshold is reached the circle status flips to Dissolved automatically.
    pub fn vote_to_dissolve(env: Env, member: Address) -> Result<(), AjoError> {
        member.require_auth();

        let status: CircleStatus = env
            .storage()
            .instance()
            .get(&DataKey::CircleStatus)
            .unwrap_or(CircleStatus::Active);

        if status != CircleStatus::VotingForDissolution {
            return Err(AjoError::NoActiveVote);
        }

        // Caller must be a registered member
        let members: Map<Address, MemberData> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .ok_or(AjoError::NotFound)?;

        if !members.contains_key(member.clone()) {
            return Err(AjoError::Unauthorized);
        }

        // Prevent double-voting
        let mut vote_cast: Map<Address, bool> = env
            .storage()
            .instance()
            .get(&DataKey::VoteCast)
            .unwrap_or_else(|| Map::new(&env));

        if vote_cast.get(member.clone()).unwrap_or(false) {
            return Err(AjoError::AlreadyVoted);
        }

        vote_cast.set(member.clone(), true);
        env.storage().instance().set(&DataKey::VoteCast, &vote_cast);

        let mut vote: DissolutionVote = env
            .storage()
            .instance()
            .get(&DataKey::DissolutionVote)
            .ok_or(AjoError::NoActiveVote)?;

        vote.votes_for += 1;

        // Check threshold
        let threshold_met = if vote.threshold_mode == 1 {
            // Supermajority: strictly more than 66%
            vote.votes_for * 100 > vote.total_members * 66
        } else {
            // Simple majority: strictly more than 50%
            vote.votes_for * 2 > vote.total_members
        };

        if threshold_met {
            env.storage()
                .instance()
                .set(&DataKey::CircleStatus, &CircleStatus::Dissolved);
        }

        env.storage()
            .instance()
            .set(&DataKey::DissolutionVote, &vote);

        Ok(())
    }

    /// Distribute funds back to members proportional to their contributions.
    /// Can only be called after the circle has been dissolved via voting.
    /// Returns the refund amount for the calling member.
    /// REENTRANCY PROTECTED: Follows Checks-Effects-Interactions pattern
    pub fn dissolve_and_refund(env: Env, member: Address) -> Result<i128, AjoError> {
        member.require_auth();

        // CHECKS: Validate all conditions first
        let status: CircleStatus = env
            .storage()
            .instance()
            .get(&DataKey::CircleStatus)
            .unwrap_or(CircleStatus::Active);

        if status != CircleStatus::Dissolved {
            return Err(AjoError::CircleNotActive);
        }

        let circle: CircleData = env
            .storage()
            .instance()
            .get(&DataKey::Circle)
            .ok_or(AjoError::NotFound)?;

        let mut members: Map<Address, MemberData> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .ok_or(AjoError::NotFound)?;

        let mut member_data = members.get(member.clone()).ok_or(AjoError::NotFound)?;

        // Refund = what they put in minus what they already took out
        let refund = member_data.total_contributed - member_data.total_withdrawn;

        if refund <= 0 {
            return Err(AjoError::InsufficientFunds);
        }

        // EFFECTS: Update state BEFORE external call
        member_data.total_withdrawn += refund;
        member_data.status = 2; // Exited
        members.set(member.clone(), member_data);
        env.storage().instance().set(&DataKey::Members, &members);

        // INTERACTIONS: External call happens LAST
        let token_client = token::Client::new(&env, &circle.token_address);
        token_client.transfer(&env.current_contract_address(), &member, &refund);

        Ok(refund)
    }

    /// Get the current circle status
    pub fn get_circle_status(env: Env) -> CircleStatus {
        env.storage()
            .instance()
            .get(&DataKey::CircleStatus)
            .unwrap_or(CircleStatus::Active)
    }

    /// Get the current dissolution vote state (if any)
    pub fn get_dissolution_vote(env: Env) -> Result<DissolutionVote, AjoError> {
        env.storage()
            .instance()
            .get(&DataKey::DissolutionVote)
            .ok_or(AjoError::NoActiveVote)
    }

    // ─── Emergency "Panic Button" ─────────────────────────────────────────────

    /// Admin-triggered emergency halt. Only the organizer can call this.
    /// Sets the circle status to `Panicked`, which blocks all normal operations
    /// and enables `emergency_refund()` for every member.
    pub fn panic(env: Env, admin: Address) -> Result<(), AjoError> {
        Self::require_admin(&env, &admin)?;

        let status = Self::get_circle_status(env.clone());
        if status == CircleStatus::Dissolved {
            return Err(AjoError::CircleAlreadyDissolved);
        }
        if status == CircleStatus::Panicked {
            return Err(AjoError::CirclePanicked);
        }

        env.storage()
            .instance()
            .set(&DataKey::CircleStatus, &CircleStatus::Panicked);

        Ok(())
    }

    /// Emergency refund available to any member when the circle is in `Panicked` state.
    /// Returns (total_contributed − total_withdrawn) to the caller with no penalty.
    /// REENTRANCY PROTECTED: Follows Checks-Effects-Interactions pattern
    pub fn emergency_refund(env: Env, member: Address) -> Result<i128, AjoError> {
        member.require_auth();

        // CHECKS: Validate all conditions first
        let status = Self::get_circle_status(env.clone());
        if status != CircleStatus::Panicked {
            return Err(AjoError::CircleNotActive);
        }

        let circle: CircleData = env
            .storage()
            .instance()
            .get(&DataKey::Circle)
            .ok_or(AjoError::NotFound)?;

        let mut members: Map<Address, MemberData> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .ok_or(AjoError::NotFound)?;

        let mut member_data = members.get(member.clone()).ok_or(AjoError::NotFound)?;

        let refund = member_data.total_contributed - member_data.total_withdrawn;
        if refund <= 0 {
            return Err(AjoError::InsufficientFunds);
        }

        // EFFECTS: Update state BEFORE external call
        member_data.total_withdrawn += refund;
        member_data.status = 2; // Exited
        members.set(member.clone(), member_data);
        env.storage().instance().set(&DataKey::Members, &members);

        // INTERACTIONS: External call happens LAST
        let token_client = token::Client::new(&env, &circle.token_address);
        token_client.transfer(&env.current_contract_address(), &member, &refund);

        Ok(refund)
    }

    /// Returns `true` when the circle is in emergency-halt state.
    pub fn is_panicked(env: Env) -> bool {
        Self::get_circle_status(env) == CircleStatus::Panicked
    }

    /// Returns true when a member has been marked KYC-verified by admin.
    pub fn is_kyc_verified(env: Env, member: Address) -> bool {
        let kyc: Map<Address, bool> = env
            .storage()
            .instance()
            .get(&DataKey::KycStatus)
            .unwrap_or_else(|| Map::new(&env));

        kyc.get(member).unwrap_or(false)
    }

    /// Admin updates oracle ETH/USD price and decimals.
    pub fn set_eth_usd_price(
        env: Env,
        admin: Address,
        price: i128,
        decimals: u32,
    ) -> Result<(), AjoError> {
        Self::require_admin(&env, &admin)?;

        if price <= 0 {
            return Err(AjoError::InvalidInput);
        }

        env.storage().instance().set(&DataKey::EthUsdPrice, &price);
        env.storage().instance().set(&DataKey::EthUsdDecimals, &decimals);
        Ok(())
    }

    /// Convert a USD amount to native token units using the stored ETH/USD oracle price.
    /// `usd_amount` should use the same decimal scale as the oracle feed.
    pub fn native_amount_for_usd(env: Env, usd_amount: i128) -> Result<i128, AjoError> {
        if usd_amount <= 0 {
            return Err(AjoError::InvalidInput);
        }

        let price: i128 = env
            .storage()
            .instance()
            .get(&DataKey::EthUsdPrice)
            .ok_or(AjoError::PriceUnavailable)?;
        let decimals: u32 = env
            .storage()
            .instance()
            .get(&DataKey::EthUsdDecimals)
            .ok_or(AjoError::PriceUnavailable)?;

        let scale = Self::pow10_checked(decimals)?;
        let numerator = usd_amount
            .checked_mul(scale)
            .ok_or(AjoError::ArithmeticOverflow)?;

        let native = numerator
            .checked_div(price)
            .ok_or(AjoError::ArithmeticOverflow)?;

        if native <= 0 {
            return Err(AjoError::InvalidInput);
        }

        Ok(native)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Address, Env, token};

    // ── Helper ────────────────────────────────────────────────────────────────

    /// Spin up a circle with the organizer + one extra member, each having
    /// contributed `contribution` tokens.
    fn setup_circle_with_member(
        env: &Env,
    ) -> (AjoCircleClient<'_>, Address, Address, Address) {
        let contract_id = env.register_contract(None, AjoCircle);
        let client = AjoCircleClient::new(env, &contract_id);

        let organizer = Address::generate(env);
        let member = Address::generate(env);
        let admin = Address::generate(env);
        let token_address = env.register_stellar_asset_contract(admin.clone());
        let token_admin = token::StellarAssetClient::new(env, &token_address);
        let token_client = token::Client::new(env, &token_address);

        // Mint tokens to participants
        token_admin.mint(&organizer, &1000_i128);
        token_admin.mint(&member, &1000_i128);

        client
            .initialize_circle(&organizer, &token_address, &100_i128, &7_u32, &12_u32, &5_u32);
        client.add_member(&organizer, &member);
        
        // Approve contract to spend tokens
        client.contribute(&organizer, &200_i128);
        client.contribute(&member, &200_i128);

        (client, organizer, member, token_address)
    }

    // ── Existing tests ────────────────────────────────────────────────────────

    #[test]
    fn enforce_member_limit_at_contract_level() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register_contract(None, AjoCircle);
        let client = AjoCircleClient::new(&env, &contract_id);

        let organizer = Address::generate(&env);
        let member_one = Address::generate(&env);
        let member_two = Address::generate(&env);
        let member_three = Address::generate(&env);
        let token_address = Address::generate(&env);

        let init = client.initialize_circle(&organizer, &token_address, &100_i128, &7_u32, &12_u32, &2_u32);
        assert_eq!(init, Ok(()));

        let first_join = client.add_member(&organizer, &member_one);
        assert_eq!(first_join, Ok(()));

        let second_join = client.add_member(&organizer, &member_two);
        assert_eq!(second_join, Ok(()));

        let third_join = client.add_member(&organizer, &member_three);
        assert_eq!(third_join, Err(AjoError::CircleAtCapacity));
    }

    // ── Panic-button tests ────────────────────────────────────────────────────

    #[test]
    fn test_panic_happy_path() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, organizer, _member, _token) = setup_circle_with_member(&env);

        // Before panic, is_panicked returns false
        assert!(!client.is_panicked());

        // Organizer triggers panic
        let result = client.panic(&organizer);
        assert_eq!(result, Ok(()));

        // Status is now Panicked
        assert!(client.is_panicked());
        assert_eq!(client.get_circle_status(), CircleStatus::Panicked);
    }

    #[test]
    fn test_panic_only_organizer() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _organizer, member, _token) = setup_circle_with_member(&env);

        // A regular member cannot trigger panic
        let result = client.panic(&member);
        assert_eq!(result, Err(AjoError::Unauthorized));
        assert!(!client.is_panicked());
    }

    #[test]
    fn test_emergency_refund_during_panic() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, organizer, member, token_address) = setup_circle_with_member(&env);
        let token_client = token::Client::new(&env, &token_address);

        // Initial balances after setup_circle_with_member: 
        // Minted 1000, contributed 200. Balance should be 800.
        assert_eq!(token_client.balance(&member), 800_i128);

        // Trigger panic
        client.panic(&organizer);

        // Member claims emergency refund
        let refund = client.emergency_refund(&member);
        assert_eq!(refund, Ok(200_i128));
        
        // Balance should now be 1000
        assert_eq!(token_client.balance(&member), 1000_i128);

        // Organizer claims emergency refund
        let org_refund = client.emergency_refund(&organizer);
        assert_eq!(org_refund, Ok(200_i128));
        assert_eq!(token_client.balance(&organizer), 1000_i128);

        // Second refund attempt fails (already withdrawn)
        let double = client.emergency_refund(&member);
        assert_eq!(double, Err(AjoError::InsufficientFunds));
    }

    #[test]
    fn test_emergency_refund_without_panic() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _organizer, member, _token) = setup_circle_with_member(&env);

        // Refund should fail when circle is not panicked
        let result = client.emergency_refund(&member);
        assert_eq!(result, Err(AjoError::CircleNotActive));
    }

    #[test]
    fn test_panic_blocks_contribute() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, organizer, member, _token) = setup_circle_with_member(&env);

        client.panic(&organizer);

        let result = client.contribute(&member, &50_i128);
        assert_eq!(result, Err(AjoError::CirclePanicked));
    }

    #[test]
    fn test_panic_blocks_join() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, organizer, _member, _token) = setup_circle_with_member(&env);

        client.panic(&organizer);

        let new_member = Address::generate(&env);
        let result = client.add_member(&organizer, &new_member);
        assert_eq!(result, Err(AjoError::CirclePanicked));
    }

    #[test]
    fn test_deposit_exact_contribution_updates_pool_and_timestamp() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, organizer, member, _token) = setup_circle_with_member(&env);

        assert_eq!(client.get_total_pool(), 0);

        let res = client.deposit(&member);
        assert_eq!(res, Ok(()));
        assert_eq!(client.get_total_pool(), 100_i128);
        assert!(client.get_last_deposit_timestamp(&member).is_ok());
    }

    #[test]
    fn test_upgrade_only_admin() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, _organizer, member, _token) = setup_circle_with_member(&env);

        let new_wasm_hash = BytesN::from_array(&env, &[0u8; 32]);
        let result = client.upgrade(&member, &new_wasm_hash);
        assert_eq!(result, Err(AjoError::Unauthorized));
    }

    #[test]
    fn test_upgrade_happy_path() {
        let env = Env::default();
        env.mock_all_auths();
        let (client, organizer, _member, _token) = setup_circle_with_member(&env);

        let new_wasm_hash = BytesN::from_array(&env, &[0u8; 32]);
        let result = client.upgrade(&organizer, &new_wasm_hash);
        assert_eq!(result, Ok(()));
    }
}

