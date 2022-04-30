# Get session info

Get info of a created session

**URL** : `/v1/sessions`

**Method** : `GET`

**Header** :
```json
{
  "Authorization": "10700afffb450068e7daf47f82d6c85e5f9a419685c0b1fda2ff0f270ec13742"
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

**Not authorized** :

The found session is not yet authorized

**Session not found**

The requested session id is non existant
