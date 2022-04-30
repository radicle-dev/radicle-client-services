# Create unauthorized session

Create an unauthorized sessions

**URL** : `/v1/sessions`

**Method** : `POST`

## Success Response

**Code** : `200 OK`

**Content examples**

This route creates an unauthorized session and returns the generated nonce and session id.

```json
{
    "id": "4d922bb0ac046b499bf4b479337fb07931b9ee4c174bdc02a4b05945bfb38d2a",
    "nonce": "AiUFrcN5dQI"
}
```
