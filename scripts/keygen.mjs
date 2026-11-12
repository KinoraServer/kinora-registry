#!/usr/bin/env node
// Make a signing key for a registry: `node scripts/keygen.mjs`.
// The private key goes in the repository secret REGISTRY_SIGNING_KEY and
// nowhere else; the public key is what Kinora servers pin.
import { generateKeyPairSync } from "node:crypto";
import { keyId, privateKeyFromSeed, publicKeyBase64 } from "./lib.mjs";

const { privateKey } = generateKeyPairSync("ed25519");
const seed = privateKey.export({ format: "der", type: "pkcs8" }).subarray(-32).toString("base64");
const publicKey = publicKeyBase64(privateKeyFromSeed(seed));

console.log(`Private key (keep secret; repository secret REGISTRY_SIGNING_KEY):\n  ${seed}\n`);
console.log(`Public key (pin this in Kinora):\n  ${publicKey}\n`);
console.log(`Key id: ${keyId(publicKey)}`);
