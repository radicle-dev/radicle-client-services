# Show blob of a file on a specific commit

Show blob of a file on a specific commit

**URL** : `/v1/projects/{{urn}}/blob/{{sha}}/{{path}}`

**Method** : `GET`

**Params** :

| Parameter    | Type     | Required?  | Description                                     |
| -------------|----------|------------|-------------------------------------------------|
| `highlight`  | boolean  | required   | Define if file should be syntax highlighted     |

## Success Response

**Code** : `200 OK`

**Content examples**

This route returns a listing off all commits on a project

```json
{
  "binary": false,
  "content": "{\n  \"name\": \"@sebastinez/web3conv\",\n  \"version\": \"1.2.0\",\n  \"description\": \"CLI App for type conversion in the cripto universe\",\n  \"main\": \"web3conv\"}",
  "html": false,
  "info": {
    "lastCommit": {
      "author": {
        "email": "smartinez@nuclearis.com",
        "name": "sebastinez"
      },
      "committer": {
        "email": "smartinez@nuclearis.com",
        "name": "sebastinez"
      },
      "committerTime": 1621126625,
      "description": "",
      "sha1": "ada4868b74d906f055773409599cad018adb0cae",
      "summary": "feat: add prettier and testing"
    },
    "name": "package.json",
    "objectType": "BLOB"
  },
  "path": "package.json"
}
```

**Notes** :

* Currently we support various languages for code highlight, but there are still some missing.
