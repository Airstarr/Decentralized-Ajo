#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::Address as _,
    token, Address, Env,
};

// ── helpers ──────────────────────────────────────────────────────────────────

/// Deploy a Stellar asset contract, return (token_id, StellarAssetClient).
fn create_token(env: &Env, admin: &Address) -> (Address, token::StellarAssetClient) {
    let token_id = env.register_stellar_asset_contract(admin.clone());
    let sac = token::StellarAssetClient::new(env, &token_id);
    (token_id, sac)
}

/// Set up a circle with `n` members, fund the contract with the full pot.
/// Returns (contract_id, token_id, organizer, members_vec).
fn setup_circle(
    env: &Env,
    n: u32,
    contribution_amount: i128,
) -> (Address, Address, Address, soroban_sdk::Vec<Address>) {
    let contract_id = env.register_contract(None, AjoCircle);
    let client = AjoCircleClient::new(env, &contract_id);

    let organizer = Address::generate(env);
    let (token_id, sac) = create_token(env, &organizer);

    client.initialize_circle(&organizer, &contribution_amount, &7_u32, &n);

    let mut members = soroban_sdk::Vec::new(env);
    members.push_back(organizer.clone());

    for _ in 1..n {
        let m = Address::generate(env);
        client.add_member(&organizer, &m);
        members.push_back(m.clone());
    }

    // Fund the contract with the full pot so token transfers succeed
    let total_pot = (n as i128) * contribution_amount;
    sac.mint(&contract_id, &total_pot);

    (contract_id, token_id, organizer, members)
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// 1% fee on a 1000-token pot (10 members × 100).
/// fee = 10, net = 990.
#[test]
fn test_fee_1pct_round_pot() {
    let env = Env::default();
    env.mock_all_auths();

    let (contract_id, token_id, organizer, members) = setup_circle(&env, 10, 100);
    let client = AjoCircleClient::new(&env, &contract_id);
    let treasury = Address::generate(&env);

    client.set_fee_config(&organizer, &treasury, &100_u32); // 1%

    let token_client = token::Client::new(&env, &token_id);
    let recipient = members.get(0).unwrap();
    let net = client.claim_payout(&recipient, &token_id);

    assert_eq!(net, 990_i128);
    assert_eq!(token_client.balance(&treasury), 10_i128);
    assert_eq!(token_client.balance(&recipient), 990_i128);
}

/// 1% fee on a 999-token pot (3 members × 333).
/// fee = floor(999 * 100 / 10000) = 9, net = 999 - 9 = 990.
/// Verifies rounding leaves zero dust in the contract.
#[test]
fn test_fee_1pct_odd_pot() {
    let env = Env::default();
    env.mock_all_auths();

    let (contract_id, token_id, organizer, members) = setup_circle(&env, 3, 333);
    let client = AjoCircleClient::new(&env, &contract_id);
    let treasury = Address::generate(&env);

    client.set_fee_config(&organizer, &treasury, &100_u32); // 1%

    let token_client = token::Client::new(&env, &token_id);
    let recipient = members.get(0).unwrap();
    let net = client.claim_payout(&recipient, &token_id);

    assert_eq!(net, 990_i128);
    assert_eq!(token_client.balance(&treasury), 9_i128);
    assert_eq!(token_client.balance(&recipient), 990_i128);
    // Zero dust: 999 - 9 - 990 = 0
    assert_eq!(token_client.balance(&contract_id), 0_i128);
}

/// Updating fee_bps via set_fee_config correctly affects the next payout.
/// First set 2%, then update to 5% — payout should reflect 5%.
#[test]
fn test_fee_config_update_affects_payout() {
    let env = Env::default();
    env.mock_all_auths();

    // 2 members × 500 = 1000 pot
    let (contract_id, token_id, organizer, members) = setup_circle(&env, 2, 500);
    let client = AjoCircleClient::new(&env, &contract_id);
    let treasury = Address::generate(&env);

    client.set_fee_config(&organizer, &treasury, &200_u32); // 2%
    client.set_fee_config(&organizer, &treasury, &500_u32); // update to 5%

    let token_client = token::Client::new(&env, &token_id);
    let recipient = members.get(0).unwrap();
    let net = client.claim_payout(&recipient, &token_id);

    // fee = floor(1000 * 500 / 10000) = 50, net = 950
    assert_eq!(net, 950_i128);
    assert_eq!(token_client.balance(&treasury), 50_i128);
    assert_eq!(token_client.balance(&recipient), 950_i128);
}

/// fee_bps = 0 must not crash and must pass the full pot to the recipient.
#[test]
fn test_zero_fee_no_crash() {
    let env = Env::default();
    env.mock_all_auths();

    // 5 members × 200 = 1000 pot
    let (contract_id, token_id, organizer, members) = setup_circle(&env, 5, 200);
    let client = AjoCircleClient::new(&env, &contract_id);
    let treasury = Address::generate(&env);

    client.set_fee_config(&organizer, &treasury, &0_u32); // 0%

    let token_client = token::Client::new(&env, &token_id);
    let recipient = members.get(0).unwrap();
    let net = client.claim_payout(&recipient, &token_id);

    assert_eq!(net, 1000_i128);
    assert_eq!(token_client.balance(&treasury), 0_i128);
    assert_eq!(token_client.balance(&recipient), 1000_i128);
}
