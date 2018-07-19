//! A _synchronous_ algorithm for dealerless distributed key generation.
//!
//! This protocol is meant to run in a _completely synchronous_ setting where each node handles all
//! messages in the same order. It can e.g. exchange messages as transactions on top of
//! `HoneyBadger`, or it can run "on-chain", i.e. committing its messages to a blockchain.
//!
//! Its messages are encrypted where necessary, so they can be publicly broadcast.
//!
//! When the protocol completes, every node receives a secret key share suitable for threshold
//! signatures and encryption. The secret master key is not known by anyone. The protocol succeeds
//! if up to _t_ nodes are faulty, where _t_ is the `threshold` parameter. The number of nodes must
//! be at least _2 t + 1_.
//!
//! ## Usage
//!
//! Before beginning the threshold key generation process, each validator needs to generate a
//! regular (non-threshold) key pair and multicast its public key. `SyncKeyGen::new` returns the
//! instance itself and a `Propose` message, containing a contribution to the new threshold keys.
//! It needs to be sent to all nodes. `SyncKeyGen::handle_propose` in turn produces an `Accept`
//! message, which is also multicast.
//!
//! All nodes must handle the exact same set of `Propose` and `Accept` messages. In this sense the
//! algorithm is synchronous: If Alice's `Accept` was handled by Bob but not by Carol, Bob and
//! Carol could receive different public key sets, and secret key shares that don't match. One way
//! to ensure this is to commit the messages to a public ledger before handling them, e.g. by
//! feeding them to a preexisting instance of Honey Badger. The messages will then appear in the
//! same order for everyone.
//!
//! To complete the process, call `SyncKeyGen::generate`. It produces your secret key share and the
//! public key set.
//!
//! While not asynchronous, the algorithm is fault tolerant: It is not necessary to handle a
//! `Propose` and all `Accept` messages from every validator. A `Propose` is _complete_ if it
//! received at least _2 t + 1_ valid `Accept`s. Only complete `Propose`s are used for key
//! generation in the end, and as long as at least one complete `Propose` is from a correct node,
//! the new key set is secure. You can use `SyncKeyGen::is_ready` to check whether at least
//! _t + 1_ `Propose`s are complete. So all nodes can call `generate` as soon as `is_ready` returns
//! `true`.
//!
//! Alternatively, you can use any stronger criterion, too, as long as all validators call
//! `generate` at the same point, i.e. after handling the same set of messages.
//! `SyncKeyGen::count_complete` returns the number of complete `Propose` messages. And
//! `SyncKeyGen::is_node_ready` can be used to check whether a particluar node's `Propose` is
//! complete.
//!
//! Finally, observer nodes can also use `SyncKeyGen`. For observers, no `Propose` and `Accept`
//! messages will be created and they do not need to send anything. On completion, they will only
//! receive the public key set, but no secret key share.
//!
//! ## Example
//!
//! ```
//! extern crate rand;
//! extern crate hbbft;
//!
//! use std::collections::BTreeMap;
//!
//! use hbbft::crypto::{PublicKey, SecretKey, SignatureShare};
//! use hbbft::sync_key_gen::{ProposeOutcome, SyncKeyGen};
//!
//! // Two out of four shares will suffice to sign or encrypt something.
//! let (threshold, node_num) = (1, 4);
//!
//! // Generate individual key pairs for encryption. These are not suitable for threshold schemes.
//! let sec_keys: Vec<SecretKey> = (0..node_num).map(|_| rand::random()).collect();
//! let pub_keys: BTreeMap<usize, PublicKey> = sec_keys
//!     .iter()
//!     .map(SecretKey::public_key)
//!     .enumerate()
//!     .collect();
//!
//! // Create the `SyncKeyGen` instances. The constructor also outputs the proposal that needs to
//! // be sent to all other participants, so we save the proposals together with their sender ID.
//! let mut nodes = BTreeMap::new();
//! let mut proposals = Vec::new();
//! for (id, sk) in sec_keys.into_iter().enumerate() {
//!     let (sync_key_gen, opt_proposal) = SyncKeyGen::new(&id, sk, pub_keys.clone(), threshold);
//!     nodes.insert(id, sync_key_gen);
//!     proposals.push((id, opt_proposal.unwrap())); // Would be `None` for observer nodes.
//! }
//!
//! // All nodes now handle the proposals and send the resulting `Accept` messages.
//! let mut accepts = Vec::new();
//! for (sender_id, proposal) in proposals {
//!     for (&id, node) in &mut nodes {
//!         match node.handle_propose(&sender_id, proposal.clone()) {
//!             Some(ProposeOutcome::Valid(accept)) => accepts.push((id, accept)),
//!             Some(ProposeOutcome::Invalid(faults)) => panic!("Invalid proposal: {:?}", faults),
//!             None => panic!("We are not an observer, so we should send Accept."),
//!         }
//!     }
//! }
//!
//! // Finally, we handle all the `Accept`s.
//! for (sender_id, accept) in accepts {
//!     for node in nodes.values_mut() {
//!         node.handle_accept(&sender_id, accept.clone());
//!     }
//! }
//!
//! // We have all the information and can generate the key sets.
//! let pub_key_set = nodes[&0].generate().0; // The public key set: identical for all nodes.
//! let mut secret_key_shares = BTreeMap::new();
//! for (&id, node) in &mut nodes {
//!     assert!(node.is_ready());
//!     let (pks, opt_sks) = node.generate();
//!     assert_eq!(pks, pub_key_set); // All nodes now know the public keys and public key shares.
//!     let sks = opt_sks.expect("Not an observer node: We receive a secret key share.");
//!     secret_key_shares.insert(id as u64, sks);
//! }
//!
//! // Three out of four nodes can now sign a message. Each share can be verified individually.
//! let msg = "Nodes 0 and 1 does not agree with this.";
//! let mut sig_shares: BTreeMap<u64, SignatureShare> = BTreeMap::new();
//! for (&id, sks) in &secret_key_shares {
//!     if id != 0 && id != 1 {
//!         let sig_share = sks.sign(msg);
//!         let pks = pub_key_set.public_key_share(id as u64);
//!         assert!(pks.verify(&sig_share, msg));
//!         sig_shares.insert(id as u64, sig_share);
//!     }
//! }
//!
//! // Two signatures are over the threshold. They are enough to produce a signature that matches
//! // the public master key.
//! let sig = pub_key_set
//!     .combine_signatures(&sig_shares)
//!     .expect("The shares can be combined.");
//! assert!(pub_key_set.public_key().verify(&sig, msg));
//! ```
//!
//! ## How it works
//!
//! The algorithm is based on ideas from
//! [Distributed Key Generation in the Wild](https://eprint.iacr.org/2012/377.pdf) and
//! [A robust threshold elliptic curve digital signature providing a new verifiable secret sharing scheme](https://www.researchgate.net/profile/Ihab_Ali/publication/4205262_A_robust_threshold_elliptic_curve_digital_signature_providing_a_new_verifiable_secret_sharing_scheme/links/02e7e538f15726323a000000/A-robust-threshold-elliptic-curve-digital-signature-providing-a-new-verifiable-secret-sharing-scheme.pdf?origin=publication_detail).
//!
//! In a trusted dealer scenario, the following steps occur:
//!
//! 1. Dealer generates a `BivarPoly` of degree _t_ and publishes the `BivarCommitment` which is
//!    used to publicly verify the polynomial's values.
//! 2. Dealer sends _row_ _m > 0_ to node number _m_.
//! 3. Node _m_, in turn, sends _value_ number _s_ to node number _s_.
//! 4. This process continues until _2 t + 1_ nodes confirm they have received a valid row. If
//!    there are at most _t_ faulty nodes, we know that at least _t + 1_ correct nodes sent on an
//!    entry of every other node's column to that node.
//! 5. This means every node can reconstruct its column, and the value at _0_ of its column.
//! 6. These values all lie on a univariate polynomial of degree _t_ and can be used as secret keys.
//!
//! In our _dealerless_ environment, at least _t + 1_ nodes each generate a polynomial using the
//! method above. The sum of the secret keys we received from each node is then used as our secret
//! key. No single node knows the secret master key.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Debug, Formatter};

use bincode;
use pairing::bls12_381::{Fr, G1Affine};
use pairing::{CurveAffine, Field};
use rand::OsRng;

use crypto::poly::{BivarCommitment, BivarPoly, Poly};
use crypto::serde_impl::field_vec::FieldWrap;
use crypto::{Ciphertext, PublicKey, PublicKeySet, SecretKey, SecretKeyShare};
use fault_log::{FaultKind, FaultLog};

// TODO: No need to send our own row and value to ourselves.

/// A submission by a validator for the key generation. It must to be sent to all participating
/// nodes and handled by all of them, including the one that produced it.
///
/// The message contains a commitment to a bivariate polynomial, and for each node, an encrypted
/// row of values. If this message receives enough `Accept`s, it will be used as summand to produce
/// the the key set in the end.
#[derive(Deserialize, Serialize, Clone, Hash, Eq, PartialEq)]
pub struct Propose(BivarCommitment, Vec<Ciphertext>);

impl Debug for Propose {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let deg = self.0.degree();
        let len = self.1.len();
        write!(f, "Propose(<degree {}>, <{} rows>)", deg, len)
    }
}

/// A confirmation that we have received and verified a validator's proposal. It must be sent to
/// all participating nodes and handled by all of them, including ourselves.
///
/// The message is only produced after we verified our row against the commitment in the `Propose`.
/// For each node, it contains one encrypted value of that row.
#[derive(Deserialize, Serialize, Clone, Hash, Eq, PartialEq)]
pub struct Accept(u64, Vec<Ciphertext>);

impl Debug for Accept {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "Accept({}, <{} values>", self.0, self.1.len())
    }
}

/// The information needed to track a single proposer's secret sharing process.
struct ProposalState {
    /// The proposer's commitment.
    commit: BivarCommitment,
    /// The verified values we received from `Accept` messages.
    values: BTreeMap<u64, Fr>,
    /// The nodes which have accepted this proposal, valid or not.
    accepts: BTreeSet<u64>,
}

impl ProposalState {
    /// Creates a new proposal state with a commitment.
    fn new(commit: BivarCommitment) -> ProposalState {
        ProposalState {
            commit,
            values: BTreeMap::new(),
            accepts: BTreeSet::new(),
        }
    }

    /// Returns `true` if at least `2 * threshold + 1` nodes have accepted.
    fn is_complete(&self, threshold: usize) -> bool {
        self.accepts.len() > 2 * threshold
    }
}

/// The outcome of handling and verifying a `Propose` message.
pub enum ProposeOutcome<NodeUid: Clone> {
    /// The message was valid: the part of it that was encrypted to us matched the public
    /// commitment, so we can multicast an `Accept` message for it.
    Valid(Accept),
    // If the Propose message passed to `handle_propose()` is invalid, the
    // fault is logged and passed onto the caller.
    /// The message was invalid: the part encrypted to us was malformed or didn't match the
    /// commitment. We now know that the proposer is faulty, and dont' send an `Accept`.
    Invalid(FaultLog<NodeUid>),
}

/// A synchronous algorithm for dealerless distributed key generation.
///
/// It requires that all nodes handle all messages in the exact same order.
pub struct SyncKeyGen<NodeUid> {
    /// Our node index.
    our_idx: Option<u64>,
    /// Our secret key.
    sec_key: SecretKey,
    /// The public keys of all nodes, by node index.
    pub_keys: BTreeMap<NodeUid, PublicKey>,
    /// Proposed bivariate polynomial.
    proposals: BTreeMap<u64, ProposalState>,
    /// The degree of the generated polynomial.
    threshold: usize,
}

impl<NodeUid: Ord + Clone + Debug> SyncKeyGen<NodeUid> {
    /// Creates a new `SyncKeyGen` instance, together with the `Propose` message that should be
    /// multicast to all nodes.
    ///
    /// If we are not a validator but only an observer, no `Propose` message is produced and no
    /// messages need to be sent.
    pub fn new(
        our_uid: &NodeUid,
        sec_key: SecretKey,
        pub_keys: BTreeMap<NodeUid, PublicKey>,
        threshold: usize,
    ) -> (SyncKeyGen<NodeUid>, Option<Propose>) {
        let our_idx = pub_keys
            .keys()
            .position(|uid| uid == our_uid)
            .map(|idx| idx as u64);
        let key_gen = SyncKeyGen {
            our_idx,
            sec_key,
            pub_keys,
            proposals: BTreeMap::new(),
            threshold,
        };
        if our_idx.is_none() {
            return (key_gen, None); // No proposal: we are an observer.
        }
        let mut rng = OsRng::new().expect("OS random number generator");
        let our_proposal = BivarPoly::random(threshold, &mut rng);
        let commit = our_proposal.commitment();
        let encrypt = |(i, pk): (usize, &PublicKey)| {
            let row = our_proposal.row(i as u64 + 1);
            let bytes = bincode::serialize(&row).expect("failed to serialize row");
            pk.encrypt(&bytes)
        };
        let rows: Vec<_> = key_gen.pub_keys.values().enumerate().map(encrypt).collect();
        (key_gen, Some(Propose(commit, rows)))
    }

    /// Handles a `Propose` message. If it is valid, returns an `Accept` message to be broadcast.
    ///
    /// If we are only an observer, `None` is returned instead and no messages need to be sent.
    pub fn handle_propose(
        &mut self,
        sender_id: &NodeUid,
        Propose(commit, rows): Propose,
    ) -> Option<ProposeOutcome<NodeUid>> {
        let sender_idx = self.node_index(sender_id)?;
        let opt_commit_row = self.our_idx.map(|idx| commit.row(idx + 1));
        match self.proposals.entry(sender_idx) {
            Entry::Occupied(_) => return None, // Ignore multiple proposals.
            Entry::Vacant(entry) => {
                entry.insert(ProposalState::new(commit));
            }
        }
        // If we are only an observer, return `None`. We don't need to send `Accept`.
        let our_idx = self.our_idx?;
        let commit_row = opt_commit_row?;
        let ser_row = self.sec_key.decrypt(rows.get(our_idx as usize)?)?;
        let row: Poly = if let Ok(row) = bincode::deserialize(&ser_row) {
            row
        } else {
            // Log the faulty node and ignore invalid messages.
            let fault_log = FaultLog::init(sender_id.clone(), FaultKind::InvalidProposeMessage);
            return Some(ProposeOutcome::Invalid(fault_log));
        };
        if row.commitment() != commit_row {
            debug!("Invalid proposal from node {}.", sender_idx);
            let fault_log = FaultLog::init(sender_id.clone(), FaultKind::InvalidProposeMessage);
            return Some(ProposeOutcome::Invalid(fault_log));
        }
        // The row is valid: now encrypt one value for each node.
        let encrypt = |(idx, pk): (usize, &PublicKey)| {
            let val = row.evaluate(idx as u64 + 1);
            let wrap = FieldWrap::new(val);
            // TODO: Handle errors.
            let ser_val = bincode::serialize(&wrap).expect("failed to serialize value");
            pk.encrypt(ser_val)
        };
        let values = self.pub_keys.values().enumerate().map(encrypt).collect();
        Some(ProposeOutcome::Valid(Accept(sender_idx, values)))
    }

    /// Handles an `Accept` message.
    pub fn handle_accept(&mut self, sender_id: &NodeUid, accept: Accept) -> FaultLog<NodeUid> {
        let mut fault_log = FaultLog::new();
        if let Some(sender_idx) = self.node_index(sender_id) {
            if let Err(err) = self.handle_accept_or_err(sender_idx, accept) {
                debug!("Invalid accept from node {}: {}", sender_idx, err);
                fault_log.append(sender_id.clone(), FaultKind::InvalidAcceptMessage);
            }
        }
        fault_log
    }

    /// Returns the number of complete proposals. If this is at least `threshold + 1`, the keys can
    /// be generated, but it is possible to wait for more to increase security.
    pub fn count_complete(&self) -> usize {
        self.proposals
            .values()
            .filter(|proposal| proposal.is_complete(self.threshold))
            .count()
    }

    /// Returns `true` if the proposal of the given node is complete.
    pub fn is_node_ready(&self, proposer_id: &NodeUid) -> bool {
        self.node_index(proposer_id)
            .and_then(|proposer_idx| self.proposals.get(&proposer_idx))
            .map_or(false, |proposal| proposal.is_complete(self.threshold))
    }

    /// Returns `true` if enough proposals are complete to safely generate the new key.
    pub fn is_ready(&self) -> bool {
        self.count_complete() > self.threshold
    }

    /// Returns the new secret key share and the public key set.
    ///
    /// These are only secure if `is_ready` returned `true`. Otherwise it is not guaranteed that
    /// none of the nodes knows the secret master key.
    ///
    /// If we are only an observer node, no secret key share is returned.
    pub fn generate(&self) -> (PublicKeySet, Option<SecretKeyShare>) {
        let mut pk_commit = Poly::zero().commitment();
        let mut opt_sk_val = self.our_idx.map(|_| Fr::zero());
        let is_complete = |proposal: &&ProposalState| proposal.is_complete(self.threshold);
        for proposal in self.proposals.values().filter(is_complete) {
            pk_commit += proposal.commit.row(0);
            if let Some(sk_val) = opt_sk_val.as_mut() {
                let row: Poly = Poly::interpolate(proposal.values.iter().take(self.threshold + 1));
                sk_val.add_assign(&row.evaluate(0));
            }
        }
        let opt_sk = opt_sk_val.map(SecretKeyShare::from_value);
        (pk_commit.into(), opt_sk)
    }

    /// Handles an `Accept` message or returns an error string.
    fn handle_accept_or_err(
        &mut self,
        sender_idx: u64,
        Accept(proposer_idx, values): Accept,
    ) -> Result<(), String> {
        if values.len() != self.pub_keys.len() {
            return Err("wrong node count".to_string());
        }
        let proposal = self
            .proposals
            .get_mut(&proposer_idx)
            .ok_or_else(|| "sender does not exist".to_string())?;
        if !proposal.accepts.insert(sender_idx) {
            return Err("duplicate accept".to_string());
        }
        let our_idx = match self.our_idx {
            Some(our_idx) => our_idx,
            None => return Ok(()), // We are only an observer. Nothing to decrypt for us.
        };
        let ser_val: Vec<u8> = self
            .sec_key
            .decrypt(&values[our_idx as usize])
            .ok_or_else(|| "value decryption failed".to_string())?;
        let val = bincode::deserialize::<FieldWrap<Fr, Fr>>(&ser_val)
            .map_err(|err| format!("deserialization failed: {:?}", err))?
            .into_inner();
        if proposal.commit.evaluate(our_idx + 1, sender_idx + 1) != G1Affine::one().mul(val) {
            return Err("wrong value".to_string());
        }
        proposal.values.insert(sender_idx + 1, val);
        Ok(())
    }

    /// Returns the index of the node, or `None` if it is unknown.
    fn node_index(&self, node_id: &NodeUid) -> Option<u64> {
        if let Some(node_idx) = self.pub_keys.keys().position(|uid| uid == node_id) {
            Some(node_idx as u64)
        } else {
            debug!("Unknown node {:?}", node_id);
            None
        }
    }
}
