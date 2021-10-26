use std::{collections::BTreeMap, convert::TryInto, time::Duration};

use librad::{net::peer::MembershipInfo, PeerId};
use outflux::{Bucket, FieldValue, Measurement};

use crate::client;
use crate::error::Error;

fn make_membership_measurement(
    this_peer_id: PeerId,
    membership: MembershipInfo,
) -> Result<Measurement, Error> {
    let mut fields: BTreeMap<String, FieldValue> = Default::default();
    let active: u64 = membership.active.len().try_into()?;
    let passive: u64 = membership.passive.len().try_into()?;
    fields.insert("active".to_string(), FieldValue::UInteger(active));
    fields.insert("passive".to_string(), FieldValue::UInteger(passive));

    let mut tags: BTreeMap<String, String> = Default::default();
    tags.insert("peer_id".to_string(), this_peer_id.default_encoding());

    let measurement = Measurement::builder("membership")
        .fields(fields)
        .tags(tags)
        .build()?;
    Ok(measurement)
}

fn make_peers_measurement(this_peer_id: PeerId, peers: &[PeerId]) -> Result<Measurement, Error> {
    let connected: u64 = peers.len().try_into()?;

    let mut fields: BTreeMap<String, FieldValue> = Default::default();
    fields.insert("connected".to_string(), FieldValue::UInteger(connected));

    let mut tags: BTreeMap<String, String> = Default::default();
    tags.insert("peer_id".to_string(), this_peer_id.default_encoding());

    let measurement = Measurement::builder("peers")
        .fields(fields)
        .tags(tags)
        .build()?;

    Ok(measurement)
}

pub async fn report_metrics_periodically(
    bucket: Bucket,
    handle: client::Handle,
    this_peer_id: PeerId,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(15));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;

        let (membership, peers) = tokio::join!(handle.get_membership(), handle.get_peers());

        let mut measurements: Vec<Measurement> = Default::default();
        let membership = membership
            .map_err(Error::from)
            .and_then(|membership| make_membership_measurement(this_peer_id, membership));
        match membership {
            Ok(point) => measurements.push(point),
            Err(e) => tracing::error!("Could not get membership info: {:?}", e),
        };

        let peers = peers
            .map_err(Error::from)
            .and_then(|peers| make_peers_measurement(this_peer_id, &peers));
        match peers {
            Ok(point) => measurements.push(point),
            Err(e) => tracing::error!("Could not get peers info: {:?}", e),
        };

        if measurements.is_empty() {
            continue;
        }

        if let Err(e) = bucket.write(&measurements, Duration::from_secs(10)).await {
            tracing::error!("Could not send metrics: {:?}", e);
        }
    }
}
