#![no_std]

use soroban_sdk::{contract, contractimpl, contracttype, token, Env, Address, Map, Vec};

#[cfg(test)]
mod test;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AjoError {
    NotFound = 1,
    Unauthorized = 2,
    AlreadyExists = 3,
    InvalidInput = 4,
    AlreadyPaid = 5,
    InsufficientFunds = 6,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CircleData {
    pub organizer: Address,
    pub contribution_amount: i128,
    pub frequency_days: u32,
    pub max_rounds: u32,
    pub current_round: u32,
    pub member_count: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemberData {
    pub address: Address,
    pub total_contributed: i128,
    pub total_withdrawn: i128,
    pub has_received_payout: bool,
    pub status: u32, // 0 = Active, 1 = Inactive, 2 = Exited
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeeConfig {
    pub treasury: Address,
    pub fee_bps: u32,
}

#[contracttype]
pub enum DataKey {
    Circle,
    Members,
    FeeConfig,
}

#[contract]
pub struct AjoCircle;

#[contractimpl]
impl AjoCircle {
    /// Set platform fee configuration. Only callable by the admin/organizer.
    /// fee_bps is in basis points (e.g. 100 = 1%).
    pub fn set_fee_config(
        env: Env,
        admin: Address,
        treasury: Address,
        fee_bps: u32,
    ) -> Result<(), AjoError> {
        admin.require_auth();

        // Validate: fee cannot exceed 100% (10000 bps)
        if fee_bps > 10_000 {
            return Err(AjoError::InvalidInput);
        }

        // Ensure caller is the circle organizer
        let circle: CircleData = env.storage()
            .instance()
            .get(&DataKey::Circle)
            .ok_or(AjoError::NotFound)?;

        if circle.organizer != admin {
            return Err(AjoError::Unauthorized);
        }

        env.storage().instance().set(
            &DataKey::FeeConfig,
            &FeeConfig { treasury, fee_bps },
        );

        Ok(())
    }

    /// Calculate fee and net payout from a total pot.
    /// Returns (fee, net_payout). Uses subtraction for net to avoid dust.
    fn calculate_fee(total_pot: i128, fee_bps: u32) -> (i128, i128) {
        let fee = (total_pot * fee_bps as i128) / 10_000_i128;
        let net_payout = total_pot - fee;
        (fee, net_payout)
    }

    /// Initialize a new Ajo circle
    pub fn initialize_circle(
        env: Env,
        organizer: Address,
        contribution_amount: i128,
        frequency_days: u32,
        max_rounds: u32,
    ) -> Result<(), AjoError> {
        organizer.require_auth();

        if contribution_amount <= 0 || frequency_days == 0 || max_rounds == 0 {
            return Err(AjoError::InvalidInput);
        }

        let circle_data = CircleData {
            organizer: organizer.clone(),
            contribution_amount,
            frequency_days,
            max_rounds,
            current_round: 1,
            member_count: 1,
        };

        env.storage().instance().set(&DataKey::Circle, &circle_data);

        let mut members: Map<Address, MemberData> = Map::new(&env);
        members.set(
            organizer.clone(),
            MemberData {
                address: organizer,
                total_contributed: 0,
                total_withdrawn: 0,
                has_received_payout: false,
                status: 0,
            },
        );

        env.storage().instance().set(&DataKey::Members, &members);

        Ok(())
    }

    /// Add a new member to the circle
    pub fn add_member(env: Env, organizer: Address, new_member: Address) -> Result<(), AjoError> {
        organizer.require_auth();

        let mut circle: CircleData = env.storage()
            .instance()
            .get(&DataKey::Circle)
            .ok_or(AjoError::NotFound)?;

        if circle.organizer != organizer {
            return Err(AjoError::Unauthorized);
        }

        let mut members: Map<Address, MemberData> = env.storage()
            .instance()
            .get(&DataKey::Members)
            .ok_or(AjoError::NotFound)?;

        if members.contains_key(new_member.clone()) {
            return Err(AjoError::AlreadyExists);
        }

        members.set(
            new_member.clone(),
            MemberData {
                address: new_member,
                total_contributed: 0,
                total_withdrawn: 0,
                has_received_payout: false,
                status: 0,
            },
        );

        circle.member_count += 1;

        env.storage().instance().set(&DataKey::Members, &members);
        env.storage().instance().set(&DataKey::Circle, &circle);

        Ok(())
    }

    /// Record a contribution from a member
    pub fn contribute(env: Env, member: Address, amount: i128) -> Result<(), AjoError> {
        member.require_auth();

        if amount <= 0 {
            return Err(AjoError::InvalidInput);
        }

        let mut members: Map<Address, MemberData> = env.storage()
            .instance()
            .get(&DataKey::Members)
            .ok_or(AjoError::NotFound)?;

        if let Some(mut member_data) = members.get(member.clone()) {
            member_data.total_contributed += amount;
            members.set(member, member_data);
        } else {
            return Err(AjoError::NotFound);
        }

        env.storage().instance().set(&DataKey::Members, &members);

        Ok(())
    }

    /// Claim payout when it's a member's turn.
    /// If a fee config is set, deducts the platform fee and transfers it to
    /// the treasury before sending the remainder to the recipient.
    /// token_address: the Stellar asset contract address used for transfers.
    pub fn claim_payout(
        env: Env,
        member: Address,
        token_address: Address,
    ) -> Result<i128, AjoError> {
        member.require_auth();

        let circle: CircleData = env.storage()
            .instance()
            .get(&DataKey::Circle)
            .ok_or(AjoError::NotFound)?;

        let mut members: Map<Address, MemberData> = env.storage()
            .instance()
            .get(&DataKey::Members)
            .ok_or(AjoError::NotFound)?;

        let mut member_data = members.get(member.clone()).ok_or(AjoError::NotFound)?;

        if member_data.has_received_payout {
            return Err(AjoError::AlreadyPaid);
        }

        let total_pot = (circle.member_count as i128) * circle.contribution_amount;

        // Resolve fee config (optional — defaults to zero fee)
        let fee_config: Option<FeeConfig> = env.storage().instance().get(&DataKey::FeeConfig);

        let (fee, net_payout) = match &fee_config {
            Some(cfg) => Self::calculate_fee(total_pot, cfg.fee_bps),
            None => (0_i128, total_pot),
        };

        let token_client = token::Client::new(&env, &token_address);
        let contract_addr = env.current_contract_address();

        // Transfer fee to treasury (skip if zero to avoid unnecessary calls)
        if fee > 0 {
            if let Some(cfg) = &fee_config {
                token_client.transfer(&contract_addr, &cfg.treasury, &fee);
            }
        }

        // Transfer net payout to recipient
        token_client.transfer(&contract_addr, &member, &net_payout);

        member_data.has_received_payout = true;
        member_data.total_withdrawn += total_pot; // record full pot as withdrawn
        members.set(member, member_data);
        env.storage().instance().set(&DataKey::Members, &members);

        Ok(net_payout)
    }

    /// Perform a partial withdrawal with penalty
    pub fn partial_withdraw(env: Env, member: Address, amount: i128) -> Result<i128, AjoError> {
        member.require_auth();

        if amount <= 0 {
            return Err(AjoError::InvalidInput);
        }

        let mut members: Map<Address, MemberData> = env.storage()
            .instance()
            .get(&DataKey::Members)
            .ok_or(AjoError::NotFound)?;

        if let Some(mut member_data) = members.get(member.clone()) {
            let available = member_data.total_contributed - member_data.total_withdrawn;

            if amount > available {
                return Err(AjoError::InsufficientFunds);
            }

            let net_amount = amount - (amount * 10) / 100;
            member_data.total_withdrawn += amount;

            members.set(member, member_data);
            env.storage().instance().set(&DataKey::Members, &members);

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
        let members: Map<Address, MemberData> = env.storage()
            .instance()
            .get(&DataKey::Members)
            .ok_or(AjoError::NotFound)?;

        members.get(member).ok_or(AjoError::NotFound)
    }

    /// Get all members
    pub fn get_members(env: Env) -> Result<Vec<MemberData>, AjoError> {
        let members: Map<Address, MemberData> = env.storage()
            .instance()
            .get(&DataKey::Members)
            .ok_or(AjoError::NotFound)?;

        let mut members_vec = Vec::new(&env);
        for (_, member) in members.iter() {
            members_vec.push_back(member);
        }

        Ok(members_vec)
    }

    /// Get current fee configuration
    pub fn get_fee_config(env: Env) -> Option<FeeConfig> {
        env.storage().instance().get(&DataKey::FeeConfig)
    }
}
