# Update session

Update an unauthorized session to an authorized one

**URL** : `/v1/sessions/{{sessionId}}`

**Method** : `PUT`

**Body** :
```json
{
  "message": "seed.sebastinez.dev wants you to sign in with your Ethereum account:\n0x5E813e48a81977c6Fdd565ed5097eb600C73C4f0\n\nseed.sebastinez.dev wants you to sign in with Ethereum\n\nURI: http://localhost:3000\nVersion: 1\nChain ID: 1\nNonce: dk560RLJtI8\nIssued At: 2022-04-29T17:45:36.752Z\nExpiration Time: 2022-05-05T22:00:00.000Z",
  "signature": "16f79d872423815e283fb9563da4ef5fd00c40b10e25f3977734606d3fc989502c9680b6b667fc4fc153d522efbd880d21a9165964234650b2939aef47707f501b"
}
```

## Success Response

**Code** : `200 OK`

**Content examples**

This route returns the generated nonce and session id.

```json
{
  "id": "10700afffb450068e7daf47f82d6c85e5f9a419685c0b1fda2ff0f270ec13742",
  "session": {
    "domain": "seed.sebastinez.dev",
    "address": "0x5e813e48a81977c6fdd565ed5097eb600c73c4f0",
    "statement": "seed.sebastinez.dev wants you to sign in with Ethereum",
    "uri": "http://localhost:3000",
    "version": 1,
    "chain_id": 1,
    "nonce": "dk560RLJtI8",
    "issued_at": 1651254336,
    "expiration_time": 1651788000,
    "resources": []
  }
}
```

## Failed Response

In general all session API errors return a `401 Unauthorized` code.

### Possible error messages :

**Not able to parse Siwe message**

The passed Siwe message was not valid or was not properly formatted

**RADICLE_DOMAIN is not set**

Env variable RADICLE_DOMAIN was not found

**Siwe statement not found**

The message was missing the SIWE statement

**Invalid nonce**

The nonce passed did not match with the nonce generated during session creation

**Siwe statement does not start with RADICLE_DOMAIN**

The SIWE statement has to start with the defined RADICLE_DOMAIN

**Invalid domain**

The passed domain does not match RADICLE_DOMAIN

**Not able to decode signature**

The signature passed does not match the Ethereum address or is not a valid signature

**Not able to verify Siwe message**

The verification of the SIWE message, expiration date, not before date, etc. was not successfully verified.

**Session already authorized**

The session to be authorized has already been authorized before

**Session not found**

The looked up session was not found

