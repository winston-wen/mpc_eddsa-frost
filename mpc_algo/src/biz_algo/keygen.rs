#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct KeyStore {
    pub party_key: KeyInitial,
    pub signing_key: KeyPair,
    pub valid_com_vec: Vec<KeyGenDKGCommitment>,

    pub member_id: u16,
    pub th: u16,
}

pub async fn algo_keygen(
    my_id: u16,     // My party id, within 1..=n_members
    th: u16,        // At least `th + 1` members during sign
    n_members: u16, // Number of keygen participants
    context: &str,  // Other parties challenge against this ctx
) -> Outcome<KeyStore> {
    assert_throw!(1 <= th && th <= n_members);
    assert_throw!((1..=n_members).contains(&my_id));
    let mut round: &str;

    // #region generate commitment and zkp for broadcasting
    let mut rng = OsRng;
    let party_key = KeyInitial::new(my_id, &mut rng);
    if false {
        use bip32::{Language, Mnemonic};
        let mnemonic = Mnemonic::from_entropy(party_key.u_i.to_bytes(), Language::English);
        let phrase = mnemonic.phrase().to_string();
        drop(phrase);
    }
    let _obj: _ = party_key
        .generate_shares(n_members, th, &mut rng)
        .catch_()?;
    let shares_com: SharesCommitment = _obj.0;
    let mut shares: Vec<Share> = _obj.1;

    let challenge = generate_dkg_challenge(
        my_id,
        context,          // known to all participants
        &party_key.g_u_i, // public key of shard
        &party_key.g_k,   // commitment of shard
    )
    .catch_()?;
    let sigma = &party_key.k + &party_key.u_i * challenge;

    let dkg_commitment = KeyGenDKGProposedCommitment {
        index: my_id,
        shares_commitment: shares_com,
        zkp: KeyGenZKP {
            g_k: party_key.g_k,
            sigma,
        },
    };
    println!("Generated commitments and zkp");
    // #endregion

    // #region round 1: send public commitment to coeffs and a proof of knowledge to u_i
    round = "dkg_commitment";
    send_bcast(my_id, round, &dkg_commitment).await.catch_()?;
    let mut dkg_com_vec: Vec<KeyGenDKGProposedCommitment> =
        recv_bcast(n_members, round).await.catch_()?;
    println!("Exchanged commitments");
    // #endregion

    // #region verify commitment and zkp from round 1 and construct aes keys
    let _obj: _ = KeyInitial::keygen_receive_commitments_and_validate_peers(&dkg_com_vec, &context)
        .catch_()?;
    let invalid_peer_ids: Vec<u16> = _obj.0;
    let valid_com_vec: Vec<KeyGenDKGCommitment> = _obj.1;
    assert_throw!(
        invalid_peer_ids.is_empty(),
        &format!("Invalid zkp from parties {:?}", invalid_peer_ids)
    );
    dkg_com_vec.iter_mut().for_each(|x| x.zeroize());

    let mut enc_keys: Vec<RistrettoPoint> = Vec::new();
    for i in 1..=n_members {
        if i != my_id {
            enc_keys.push(
                &valid_com_vec[i as usize - 1].shares_commitment.commitment[0] * &party_key.u_i,
            );
        }
    }
    // #endregion

    // #region round 2: send secret shares via aes-p2p
    round = "aead_pack_i";
    let mut j = 0;
    for (k, i) in (1..=n_members).enumerate() {
        if i != my_id {
            // prepare encrypted share for party i
            let key_i = &enc_keys[j].compress().to_bytes();
            let plaintext = shares[k].get_value().to_bytes();
            let aead_pack_i = aes_encrypt(key_i, &plaintext).catch_()?;
            send_p2p(my_id, i, round, &aead_pack_i).await.catch_()?;
            j += 1;
        }
    }
    let aead_vec: Vec<AEAD> = gather_p2p(my_id, n_members, round).await.catch_()?;
    println!("Finished keygen round {round}");
    // #endregion

    // #region retrieve private signing key share
    let mut j = 0;
    let mut party_shares: Vec<Share> = Vec::new();
    for i in 1..=n_members {
        if i == my_id {
            party_shares.push(shares[(i - 1) as usize].clone());
            shares.zeroize();
        } else {
            let aead_pack = aead_vec.get(j).ifnone_()?;
            let key_i = enc_keys.get(j).ifnone_()?.compress().to_bytes();
            let out = aes_decrypt(&key_i, &aead_pack).catch_()?;
            let mut out_arr = [0u8; 32];
            out_arr.copy_from_slice(&out);
            let out_fe = Share::new_from(i, my_id, Scalar::from_bytes_mod_order(out_arr));
            party_shares.push(out_fe);
            j += 1;
        }
    }

    let signing_key: KeyPair = KeyInitial::keygen_verify_share_construct_keypair(
        party_shares.clone(),
        valid_com_vec.clone(),
        my_id,
    )
    .catch_()?;
    party_shares.iter_mut().for_each(|x| x.zeroize());
    // #endregion

    let keystore = KeyStore {
        party_key,
        signing_key,
        valid_com_vec,

        member_id: my_id,
        th,
    };
    println!("Finished keygen");

    Ok(keystore)
}

use curve25519_dalek::{ristretto::RistrettoPoint, scalar::Scalar};
use mpc_sesman::{gather_p2p, recv_bcast, send_bcast, send_p2p};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::aes::*;
use crate::party_i::{
    generate_dkg_challenge, KeyGenDKGCommitment, KeyGenDKGProposedCommitment, KeyGenZKP,
    KeyInitial, KeyPair, Share, SharesCommitment,
};
use crate::prelude::*;
