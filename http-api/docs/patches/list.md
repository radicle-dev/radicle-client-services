# List all patches

List all patches on a specific seed node

**URL** : `/v1/pr`

**Method** : `GET`

## Success Response

**Code** : `200 OK`

**Content examples**

This route returns a listing of all patches on a specific seed node

```json
[
  {
    "commit": "f11f9cca4e6ad089c80166340b571c5ff94b8ca5",
    "id": "radicle-patch/feature/1",
    "mergeBase": "f11f9cca4e6ad089c80166340b571c5ff94b8ca5",
    "message": "Feature 1\n",
    "peer": {
      "delegate": true,
      "id": "hydwyyod7zet66r7x5fkckkbqp8zgpgjbnxf5rydaqfmqs3sigguwy",
      "person": {
        "name": "erikli"
      }
    }
  },
  {
    "commit": "2f744a16861fbe78b7f02b9b952ac06f86ad949a",
    "id": "radicle-patch/feature/2",
    "mergeBase": "f11f9cca4e6ad089c80166340b571c5ff94b8ca5",
    "message": "Feature 2\n",
    "peer": {
      "delegate": true,
      "id": "hydwyyod7zet66r7x5fkckkbqp8zgpgjbnxf5rydaqfmqs3sigguwy",
      "person": {
        "name": "erikli"
      }
    }
  },
]
```
