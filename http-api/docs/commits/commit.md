# Show details on a specific commit

Show the details on a specific commit

**URL** : `/v1/projects/{{urn}}/commits/{{sha}}`

**Method** : `GET`

## Success Response

**Code** : `200 OK`

**Content examples**

This route returns detailed information on a specific commit

```json
{
  "branches": [
    "main",
  ],
  "context": {
    "committer": null
  },
  "diff": {
    "copied": [],
    "created": [
      "README.md",
      "index.js",
      "package-lock.json",
      "package.json"
    ],
    "deleted": [],
    "modified": [],
    "moved": []
  },
  "header": {
    "author": {
      "email": "sebastinez@me.com",
      "name": "Sebastian Martinez"
    },
    "committer": {
      "email": "sebastinez@me.com",
      "name": "Sebastian Martinez"
    },
    "committerTime": 1644424659,
    "description": "",
    "sha1": "452cfe5255036287dc455e0b0fd75b8e767dcbca",
    "summary": "Initial commit"
  },
  "stats": {
    "additions": 0,
    "deletions": 0
  }
}
```
