// v1/ed.mjs — ed25519 with raw keys, as the protocol writes them.
//
// The spec carries keys and signatures as base64url without padding: a
// 32-byte seed for a private key, a 32-byte public key, a 64-byte
// signature. node:crypto wants DER, so a raw key is wrapped in the fixed
// PKCS#8 / SPKI prefix for Ed25519 (RFC 8410) on the way in and cut out
// on the way out. Seeds are secrets: nothing here logs or echoes one.
import { createPrivateKey, createPublicKey, generateKeyPairSync, randomBytes, sign as edSign, verify as edVerify } from "node:crypto";

const PKCS8 = Buffer.from("302e020100300506032b657004220420", "hex");
const SPKI = Buffer.from("302a300506032b6570032100", "hex");
const B64URL = /^[A-Za-z0-9_-]*$/;

/** base64url without padding. */
export const b64u = (buf) => Buffer.from(buf).toString("base64url");

/** base64url text → bytes of exactly `len` bytes, else null. */
export function fromB64u(s, len) {
  if (!B64URL.test(String(s ?? ""))) return null;
  const buf = Buffer.from(String(s), "base64url");

  return buf.length === len && b64u(buf) === s ? buf : null;
}

const privateOf = (seed) => createPrivateKey({ key: Buffer.concat([PKCS8, seed]), format: "der", type: "pkcs8" });
const publicOf = (pub) => createPublicKey({ key: Buffer.concat([SPKI, pub]), format: "der", type: "spki" });

/** A fresh key pair: {seed, pub}, both base64url. */
export function generate() {
  const { privateKey } = generateKeyPairSync("ed25519");
  const seed = privateKey.export({ format: "der", type: "pkcs8" }).subarray(PKCS8.length);

  return { seed: b64u(seed), pub: pubOf(b64u(seed)) };
}

/** The public key of a base64url seed. */
export function pubOf(seedB64) {
  const seed = fromB64u(seedB64, 32);

  if (!seed) throw new Error("a seed is 32 bytes of base64url");

  return b64u(createPublicKey(privateOf(seed)).export({ format: "der", type: "spki" }).subarray(SPKI.length));
}

/** Signature (base64url) of `bytes` by the base64url seed. */
export function sign(seedB64, bytes) {
  const seed = fromB64u(seedB64, 32);

  if (!seed) throw new Error("a seed is 32 bytes of base64url");

  return b64u(edSign(null, bytes, privateOf(seed)));
}

/** Whether `sigB64` is the base64url public key's signature of `bytes`.
 *  Malformed keys or signatures are a plain false. */
export function verify(pubB64, bytes, sigB64) {
  const pub = fromB64u(pubB64, 32);
  const sig = fromB64u(sigB64, 64);

  if (!pub || !sig) return false;

  try {
    return edVerify(null, bytes, publicOf(pub), sig);
  } catch {
    return false;
  }
}

/** 128 random bits, base64url (open ids and nonces). */
export const random128 = () => b64u(randomBytes(16));
