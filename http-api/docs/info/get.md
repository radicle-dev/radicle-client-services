# Show Seed Info

Get some metadata of the seed

**URL** : `/`

**Method** : `GET`

## Success Response

**Code** : `200 OK`

**Content examples**

This route is a welcome route for a first time visit, to point to interesting resources

```json
{
  "links": [
    {
      "href": "/v1/projects",
      "rel": "projects",
      "type": "GET"
    },
    {
      "href": "/v1/peer",
      "rel": "peer",
      "type": "GET"
    },
    {
      "href": "/v1/delegates/:urn/projects",
      "rel": "projects",
      "type": "GET"
    }
  ],
  "message": "Welcome!",
  "path": "/",
  "service": "radicle-http-api",
  "version": "0.2.0"
}
```
