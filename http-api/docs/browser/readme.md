# Show readme on a specific commit

Show readme on a specific commit

**URL** : `/v1/projects/{{urn}}/readme/{{sha}}`

**Method** : `GET`

## Success Response

**Code** : `200 OK`

**Content examples**

This route returns the README file of a project on a specific commit

```json
{
  "binary": false,
  "content": "# CLI App for Cripto Conversions",
  "html": false,
  "info": {
    "lastCommit": {
      "author": {
        "email": "sebastinez@me.com",
        "name": "Sebastian Martinez"
      },
      "committer": {
        "email": "sebastinez@me.com",
        "name": "Sebastian Martinez"
      },
      "committerTime": 1650652146,
      "description": "",
      "sha1": "9c50cac181eb1a5ef58b320d82ca0bdd489d1352",
      "summary": "Change"
    },
    "name": "README.md",
    "objectType": "BLOB"
  },
  "path": "README.md"
}
```
