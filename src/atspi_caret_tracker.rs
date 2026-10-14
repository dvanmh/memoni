use anyhow::Result;
use atspi::events::object::TextSelectionChangedEvent;
use atspi::{
    AtspiError, Event, ObjectRefOwned,
    events::object::{StateChangedEvent, TextCaretMovedEvent},
    proxy::{
        accessible::AccessibleProxy, collection::CollectionProxy, component::ComponentProxy,
        text::TextProxy,
    },
};
use futures_lite::{future, stream::StreamExt};
use log::{debug, error, trace, warn};
use mio::Waker;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, mpsc},
};
use std::{sync::Mutex, thread, time::Duration};
use zbus::{MatchRule, fdo::DBusProxy, message::Type, proxy::CacheProperties};

const CONNECTION_LIMIT: usize = 10;
const DESCENDANT_SEARCH_LIMIT: usize = 30;
const PROXY_CACHE_LIMIT: usize = 64;

pub struct EventChannel<T> {
    pub slot: Mutex<Option<T>>,
    pub waker: Waker,
}

impl<T> EventChannel<T> {
    pub fn send(&self, val: T) -> std::io::Result<()> {
        *self.slot.lock().unwrap() = Some(val);
        self.waker.wake()?;
        Ok(())
    }

    pub fn take(&self) -> Option<T> {
        self.slot.lock().unwrap().take()
    }
}

#[derive(Debug)]
pub enum CaretTrackerEvent {
    CaretPosition { x: i32, y: i32, caret_height: i32 },
    NoCaret,
    Error(atspi::AtspiError),
}

pub struct AtspiCaretTracker {}

impl AtspiCaretTracker {
    pub fn new(event_channel: Arc<EventChannel<CaretTrackerEvent>>) -> Result<Self> {
        let initial_conn = future::block_on(create_connection())?;
        thread::spawn(move || {
            let mut conn = initial_conn;
            'outer: loop {
                if let Err(err) = future::block_on(listen_for_events(&conn, event_channel.clone()))
                {
                    warn!("received critical AT-SPI error, creating new connection: {err:?}");

                    if let Ok(atspi_err) = err.downcast::<AtspiError>()
                        && let Err(err) = event_channel.send(CaretTrackerEvent::Error(atspi_err))
                    {
                        error!("failed to send AT-SPI error event: {err:?}");
                        break;
                    }

                    conn = 'blk: {
                        let mut last_err = None;
                        for i in 0..CONNECTION_LIMIT {
                            match future::block_on(create_connection()) {
                                Ok(conn) => break 'blk conn,
                                Err(err) => {
                                    debug!(
                                        "failed to create new connection: {err:?}, retry count = {i}"
                                    );
                                    last_err = Some(err);
                                    thread::sleep(Duration::from_millis(100));
                                }
                            }
                        }

                        error!("failed to create new AT-SPI connection: {last_err:?}");
                        break 'outer;
                    }
                }
            }
        });

        Ok(AtspiCaretTracker {})
    }
}

async fn create_connection() -> Result<atspi::AccessibilityConnection> {
    let conn = atspi::AccessibilityConnection::new().await?;
    conn.add_match_rule::<TextCaretMovedEvent>().await?;
    conn.add_match_rule::<TextSelectionChangedEvent>().await?;

    // conn.add_match_rule cannot filter further by state
    let dbus = DBusProxy::new(conn.connection()).await?;
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface("org.a11y.atspi.Event.Object")?
        .member("StateChanged")?
        .arg(0, "focused")?
        .build();
    dbus.add_match_rule(rule).await?;
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface("org.a11y.atspi.Event.Object")?
        .member("StateChanged")?
        .arg(0, "defunct")?
        .build();
    dbus.add_match_rule(rule).await?;

    Ok(conn)
}

async fn listen_for_events(
    conn: &atspi::AccessibilityConnection,
    event_channel: Arc<EventChannel<CaretTrackerEvent>>,
) -> Result<()> {
    let (tx, rx) = mpsc::channel();

    let mut events = conn.event_stream();
    std::pin::pin!(&mut events);

    let conn = conn.clone();
    thread::spawn(move || {
        future::block_on(async move {
            let zbus_conn = conn.connection();
            let mut contains_cache: HashMap<ObjectKey, bool> = HashMap::new();
            let mut proxy_cache = ProxyCache::new();

            let mut focused_item = match find_focused(zbus_conn, &mut proxy_cache).await {
                Ok(item) => item,
                Err(err) => {
                    debug!("failed to find initial focused item: {err:?}");
                    None
                }
            };
            debug!("initial focused item: {focused_item:?}");

            if let Some(focused_item) = &focused_item {
                match get_text_caret_position(zbus_conn, focused_item, None, &mut proxy_cache).await
                {
                    Ok(Some((x, y, caret_height))) => {
                        match event_channel.send(CaretTrackerEvent::CaretPosition {
                            x,
                            y,
                            caret_height,
                        }) {
                            Ok(()) => {}
                            Err(err) => debug!("failed to send initial caret position: {err:?}"),
                        }
                    }
                    Ok(None) => debug!("no initial caret position"),
                    Err(err) => debug!("failed to get initial caret position: {err:?}"),
                };
            }

            while let Ok(event) = rx.recv() {
                if let Err(err) = process_event(
                    zbus_conn,
                    &event_channel,
                    event,
                    &mut focused_item,
                    &mut contains_cache,
                    &mut proxy_cache,
                )
                .await
                {
                    warn!("failed to process AT-SPI event: {err:?}");
                }
            }
        });
    });

    // Drain atspi events as fast as possible so that the zbus queue
    // won't be full and hang the whole loop in case of event burst
    future::block_on(async {
        while let Some(event) = events.next().await {
            let event = event?;
            tx.send(event)?;
        }
        Ok::<_, anyhow::Error>(())
    })?;

    Ok(())
}

async fn find_focused(
    conn: &zbus::Connection,
    proxy_cache: &mut ProxyCache,
) -> zbus::Result<Option<ObjectRefOwned>> {
    let root = AccessibleProxy::builder(conn)
        .destination("org.a11y.atspi.Registry")?
        .path("/org/a11y/atspi/accessible/root")?
        .cache_properties(CacheProperties::No)
        .build()
        .await?;

    for app_ref in root.get_children().await? {
        let Ok(app) = proxy_cache.accessible(conn, &app_ref).await else {
            continue;
        };
        let Ok(windows) = app.get_children().await else {
            continue;
        };

        for win_ref in windows {
            let Ok(win) = proxy_cache.accessible(conn, &win_ref).await else {
                continue;
            };
            let Ok(states) = win.get_state().await else {
                continue;
            };
            if states.contains(atspi::State::Active) {
                return match collection_find_focused(conn, &win_ref).await {
                    Ok(found) => {
                        debug!("using Collection interface to find focused item");
                        Ok(found)
                    }
                    Err(err) => {
                        debug!("failed to find focused item via Collection interface: {err:?}");
                        debug!("recursively walking to find focused item");
                        walk_find_focused(conn, win_ref, proxy_cache).await
                    }
                };
            }
        }
    }
    Ok(None)
}

async fn process_event(
    zbus_conn: &zbus::Connection,
    event_channel: &EventChannel<CaretTrackerEvent>,
    event: Event,
    focused_item: &mut Option<ObjectRefOwned>,
    contains_cache: &mut HashMap<ObjectKey, bool>,
    proxy_cache: &mut ProxyCache,
) -> Result<()> {
    let set_caret_position =
        async |item: &ObjectRefOwned, caret_offset: Option<i32>, proxy_cache: &mut ProxyCache| {
            let result = get_text_caret_position(zbus_conn, item, caret_offset, proxy_cache).await;
            match result {
                Ok(Some((x, y, caret_height))) => {
                    event_channel.send(CaretTrackerEvent::CaretPosition { x, y, caret_height })
                }
                Ok(None) => event_channel.send(CaretTrackerEvent::NoCaret),
                Err(err) => event_channel.send(CaretTrackerEvent::Error(err)),
            }
        };

    if let Ok(ev) = TextCaretMovedEvent::try_from(event.clone()) {
        if let Some(fi) = &focused_item
            && contains_or_equals_cached(zbus_conn, fi, &ev.item, contains_cache, proxy_cache)
                .await?
        {
            debug!("received focused {:?}", ev);
            set_caret_position(&ev.item, Some(ev.position), proxy_cache).await?;
        } else {
            trace!("received unfocused {:?}", ev);
        }
        return Ok(());
    }

    if let Ok(ev) = TextSelectionChangedEvent::try_from(event.clone()) {
        if let Some(fi) = &focused_item
            && contains_or_equals_cached(zbus_conn, fi, &ev.item, contains_cache, proxy_cache)
                .await?
        {
            debug!("received focused {:?}", ev);
            set_caret_position(&ev.item, None, proxy_cache).await?;
        } else {
            trace!("received unfocused {:?}", ev);
        }
        return Ok(());
    }

    if let Ok(ev) = StateChangedEvent::try_from(event.clone()) {
        if ev.state == atspi::State::Defunct {
            debug!("received defunct {ev:?}");
            invalidate_defunct(proxy_cache, contains_cache, &ev.item);
            return Ok(());
        }

        if ev.state == atspi::State::Focused {
            if ev.enabled {
                debug!("received enabled focused {ev:?}");

                set_caret_position(&ev.item, None, proxy_cache).await?;
                *focused_item = Some(ev.item);
                contains_cache.clear();
            } else {
                trace!("received disabled focused {ev:?}");
            }
        } else {
            debug!("received non-focused {ev:?}");
        }
        return Ok(());
    }

    debug!("reveived unwanted AT-SPI event: {event:?}");
    Ok(())
}

async fn get_text_caret_position(
    conn: &zbus::Connection,
    ev_item: &ObjectRefOwned,
    caret_offset: Option<i32>,
    proxy_cache: &mut ProxyCache,
) -> Result<Option<(i32, i32, i32)>, AtspiError> {
    if is_null(ev_item) {
        debug!("event item is null");
        return Ok(None);
    }

    let item = proxy_cache.accessible(conn, ev_item).await?;

    let state = item.get_state().await?;
    if !state.contains(atspi::State::Editable) {
        debug!("event item {:?} is not editable", ev_item.path());
        return Ok(None);
    }

    let interfaces = item.get_interfaces().await?;
    if !interfaces.contains(atspi::Interface::Text) {
        debug!("event item {:?} is not text", ev_item.path());
        return Ok(None);
    }

    let (item_with_caret, item_with_caret_offset) =
        get_descendant_with_caret(conn, ev_item.clone(), proxy_cache)
            .await?
            .unzip();
    debug!("item with caret: {:?}", item_with_caret);

    let item = item_with_caret.as_ref().unwrap_or(ev_item);
    let text_proxy = proxy_cache.text(conn, item).await?;

    let char_count = text_proxy.character_count().await?;
    debug!("item character count {char_count}");

    let (x, y, h, source) = if char_count == 0 {
        let component_proxy = proxy_cache.component(conn, item).await?;
        let (x, y, _, h) = component_proxy
            .get_extents(atspi::CoordType::Screen)
            .await?;
        (x, y, h, "empty text")
    } else {
        let offset = if let Some(offset) = item_with_caret_offset {
            offset
        } else {
            match caret_offset {
                Some(offset)
                    if item.path() == ev_item.path() && (0..=char_count).contains(&offset) =>
                {
                    offset
                }
                _ => {
                    debug!("invalid event caret offset {caret_offset:?}, querying directly");
                    text_proxy.caret_offset().await?
                }
            }
        };
        let (_, start, end) = text_proxy
            .get_string_at_offset(offset, atspi::Granularity::Line)
            .await?;

        // Offset at the end of a line can have wrong extents if query directly,
        // so we step 1 step back and use that extents instead.
        let right_most_offset = if end == -1 { offset } else { end };

        if offset >= right_most_offset {
            let second_right_most_offset = right_most_offset - 1;
            if second_right_most_offset < start {
                let (x, y, _, h) = text_proxy
                    .get_character_extents(start, atspi::CoordType::Screen)
                    .await?;
                (x, y, h, "empty line")
            } else if start == -1
                && offset == char_count
                && let (srmo_line, srmo_start, _) = text_proxy
                    .get_string_at_offset(second_right_most_offset, atspi::Granularity::Line)
                    .await?
                && srmo_line.ends_with("\n")
            {
                let (_, srmo_y, _, _) = text_proxy
                    .get_character_extents(srmo_start, atspi::CoordType::Screen)
                    .await?;

                let component_proxy = proxy_cache.component(conn, item).await?;
                let (x, y, _, h) = component_proxy
                    .get_extents(atspi::CoordType::Screen)
                    .await?;

                (x, y + h, srmo_y - y, "last text line that's empty")
            } else {
                let (x, y, w, h) = text_proxy
                    .get_character_extents(second_right_most_offset, atspi::CoordType::Screen)
                    .await?;
                (x + w, y, h, "right most caret")
            }
        } else {
            let (x, y, _, h) = text_proxy
                .get_character_extents(offset, atspi::CoordType::Screen)
                .await?;
            (x, y, h, "normal")
        }
    };

    if x < 0 || y < 0 || h <= 0 {
        debug!(
            "item {:?} has invalid caret properties: x = {x}, y = {y}, caret height = {h}, source = {source}",
            item.path()
        );
        return Ok(None);
    }

    debug!(
        "item {:?} has caret properties from source = {source}",
        item.path()
    );
    Ok(Some((x, y, h)))
}

async fn get_descendant_with_caret(
    conn: &zbus::Connection,
    item: ObjectRefOwned,
    proxy_cache: &mut ProxyCache,
) -> Result<Option<(ObjectRefOwned, i32)>, AtspiError> {
    let mut queue: VecDeque<ObjectRefOwned> = VecDeque::new();
    queue.push_front(item);

    let mut count = 0;
    while let Some(item) = queue.pop_front() {
        count += 1;
        if count == DESCENDANT_SEARCH_LIMIT {
            debug!(
                "limit of {DESCENDANT_SEARCH_LIMIT} reached, stopping searching for descendant with caret"
            );
            break;
        }

        let accessible = proxy_cache.accessible(conn, &item).await?;
        let children = accessible.get_children().await?;

        if children.is_empty() {
            let text_proxy = proxy_cache.text(conn, &item).await?;

            match text_proxy.caret_offset().await {
                Ok(offset) => {
                    if offset >= 0 {
                        return Ok(Some((item, offset)));
                    } else {
                        trace!("item {:?} doesn't have caret", item.path());
                    }
                }
                Err(err) if is_unknown_error(&err) => {
                    trace!("item {:?} is not a Text object", item.path());
                }
                Err(err) => return Err(err.into()),
            }

            continue;
        }

        for child in children {
            queue.push_front(child);
        }
    }

    Ok(None)
}

async fn collection_find_focused(
    conn: &zbus::Connection,
    win: &ObjectRefOwned,
) -> zbus::Result<Option<ObjectRefOwned>> {
    let col = CollectionProxy::builder(conn)
        .destination(
            win.name()
                .ok_or(zbus::Error::Failure("missing name".to_string()))?
                .clone(),
        )?
        .path(win.path())?
        .cache_properties(CacheProperties::No)
        .build()
        .await?;

    let rule = atspi::ObjectMatchRule::builder()
        .states([atspi::State::Focused], atspi::MatchType::All)
        .build();

    Ok(col
        .get_matches(rule, atspi::SortOrder::Canonical, 1, true)
        .await?
        .into_iter()
        .find(|r| !is_null(r)))
}

async fn walk_find_focused(
    conn: &zbus::Connection,
    root: ObjectRefOwned,
    proxy_cache: &mut ProxyCache,
) -> zbus::Result<Option<ObjectRefOwned>> {
    let mut stack = vec![root];
    while let Some(r) = stack.pop() {
        let Ok(acc) = proxy_cache.accessible(conn, &r).await else {
            continue;
        };
        if let Ok(states) = acc.get_state().await
            && states.contains(atspi::State::Focused)
        {
            return Ok(Some(r));
        }

        if let Ok(children) = acc.get_children().await {
            stack.extend(children.into_iter().filter(|c| !is_null(c)));
        }
    }

    Ok(None)
}

async fn contains_or_equals_cached(
    conn: &zbus::Connection,
    item: &ObjectRefOwned,
    target: &ObjectRefOwned,
    contains_cache: &mut HashMap<ObjectKey, bool>,
    proxy_cache: &mut ProxyCache,
) -> Result<bool> {
    let key = object_key(target);
    if let Some(v) = contains_cache.get(&key) {
        return Ok(*v);
    }

    let v = contains_or_equals(conn, item, target, proxy_cache).await?;
    contains_cache.insert(key, v);
    Ok(v)
}

async fn contains_or_equals(
    conn: &zbus::Connection,
    item: &ObjectRefOwned,
    target: &ObjectRefOwned,
    proxy_cache: &mut ProxyCache,
) -> Result<bool> {
    if target.name_as_str() != item.name_as_str() {
        return Ok(false);
    }

    let mut target = target.clone();
    while target.path() != item.path() {
        let parent = proxy_cache
            .accessible(conn, &target)
            .await?
            .parent()
            .await?;
        if is_null(&parent) {
            return Ok(false);
        }

        target = parent;
    }

    Ok(true)
}

fn invalidate_defunct(
    proxy_cache: &mut ProxyCache,
    contains_cache: &mut HashMap<ObjectKey, bool>,
    item: &ObjectRefOwned,
) {
    let sender = item.name_as_str().unwrap_or_default();
    let path = item.path_as_str();
    let key = (sender.to_string(), path.to_string());

    let mut dead: HashSet<ObjectKey> = HashSet::new();
    dead.insert(key.clone());
    let mut stack = vec![key];
    while let Some(k) = stack.pop() {
        if let Some(children) = proxy_cache.children.get(&k) {
            for child in children {
                if dead.insert(child.clone()) {
                    stack.push(child.clone());
                }
            }
        }
    }

    proxy_cache.accessible.retain(|k, _| !dead.contains(k));
    proxy_cache.text.retain(|k, _| !dead.contains(k));
    proxy_cache.component.retain(|k, _| !dead.contains(k));
    proxy_cache.parents.retain(|k, _| !dead.contains(k));
    proxy_cache.children.retain(|k, set| {
        if dead.contains(k) {
            return false;
        }
        set.retain(|c| !dead.contains(c));
        !set.is_empty()
    });
    contains_cache.retain(|k, _| !dead.contains(k));
}

type ObjectKey = (String, String);
type AncestryEdge = (ObjectKey, ObjectKey);

struct ProxyCache {
    accessible: LruMap<AccessibleProxy<'static>>,
    text: LruMap<TextProxy<'static>>,
    component: LruMap<ComponentProxy<'static>>,
    parents: HashMap<ObjectKey, ObjectKey>,
    children: HashMap<ObjectKey, HashSet<ObjectKey>>,
}

impl ProxyCache {
    fn new() -> Self {
        Self {
            accessible: LruMap::new(PROXY_CACHE_LIMIT),
            text: LruMap::new(PROXY_CACHE_LIMIT),
            component: LruMap::new(PROXY_CACHE_LIMIT),
            parents: HashMap::new(),
            children: HashMap::new(),
        }
    }

    async fn accessible(
        &mut self,
        conn: &zbus::Connection,
        r: &ObjectRefOwned,
    ) -> zbus::Result<AccessibleProxy<'static>> {
        let key = object_key(r);
        if let Some(proxy) = self.accessible.get(&key) {
            return Ok(proxy.clone());
        }

        let proxy = build_accessible(conn, r).await?;
        self.record_ancestry(conn, &key, &proxy).await?;
        self.accessible.insert(key, proxy.clone());

        Ok(proxy)
    }

    async fn text(
        &mut self,
        conn: &zbus::Connection,
        item: &ObjectRefOwned,
    ) -> zbus::Result<TextProxy<'static>> {
        let key = object_key(item);
        if let Some(proxy) = self.text.get(&key) {
            return Ok(proxy.clone());
        }

        let proxy = TextProxy::builder(conn)
            .destination(
                item.name()
                    .ok_or(zbus::Error::Failure("missing name".to_string()))?
                    .clone(),
            )?
            .path(item.path())?
            .cache_properties(CacheProperties::No)
            .build()
            .await?;
        self.record_ancestry_if_missing(conn, &key, item).await?;
        self.text.insert(key, proxy.clone());

        Ok(proxy)
    }

    async fn component(
        &mut self,
        conn: &zbus::Connection,
        item: &ObjectRefOwned,
    ) -> zbus::Result<ComponentProxy<'static>> {
        let key = object_key(item);
        if let Some(proxy) = self.component.get(&key) {
            return Ok(proxy.clone());
        }

        let proxy = ComponentProxy::builder(conn)
            .destination(
                item.name()
                    .ok_or(zbus::Error::Failure("missing name".to_string()))?
                    .clone(),
            )?
            .path(item.path())?
            .cache_properties(CacheProperties::No)
            .build()
            .await?;
        self.record_ancestry_if_missing(conn, &key, item).await?;
        self.component.insert(key, proxy.clone());

        Ok(proxy)
    }

    async fn record_ancestry(
        &mut self,
        conn: &zbus::Connection,
        key: &ObjectKey,
        acc: &AccessibleProxy<'static>,
    ) -> zbus::Result<()> {
        if self.parents.contains_key(key) {
            return Ok(());
        }

        match self.walk_up_ancestry(conn, key, acc).await {
            Ok(edges) => {
                self.commit_ancestry(edges);
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    async fn record_ancestry_if_missing(
        &mut self,
        conn: &zbus::Connection,
        key: &ObjectKey,
        item: &ObjectRefOwned,
    ) -> zbus::Result<()> {
        if !self.parents.contains_key(key) {
            let acc = self.accessible(conn, item).await?;
            self.record_ancestry(conn, key, &acc).await?;
        }
        Ok(())
    }

    async fn walk_up_ancestry(
        &mut self,
        conn: &zbus::Connection,
        key: &ObjectKey,
        acc: &AccessibleProxy<'static>,
    ) -> zbus::Result<Vec<AncestryEdge>> {
        let mut edges = Vec::new();
        let mut child_key = key.clone();
        let mut cur = acc.parent().await?;

        while !is_null(&cur) {
            let parent_key = object_key(&cur);
            edges.push((child_key, parent_key.clone()));
            if self.parents.contains_key(&parent_key) {
                return Ok(edges);
            }
            child_key = parent_key;

            let parent_acc = match self.accessible.get(&child_key) {
                Some(proxy) => proxy,
                None => &build_accessible(conn, &cur).await?,
            };
            cur = parent_acc.parent().await?;
        }

        Ok(edges)
    }

    fn commit_ancestry(&mut self, edges: Vec<AncestryEdge>) {
        for (child, parent) in edges {
            self.parents.insert(child.clone(), parent.clone());
            self.children.entry(parent).or_default().insert(child);
        }
    }
}

async fn build_accessible(
    conn: &zbus::Connection,
    item: &ObjectRefOwned,
) -> zbus::Result<AccessibleProxy<'static>> {
    AccessibleProxy::builder(conn)
        .destination(
            item.name()
                .ok_or(zbus::Error::Failure("missing name".to_string()))?
                .clone(),
        )?
        .path(item.path())?
        .cache_properties(CacheProperties::No)
        .build()
        .await
}

fn object_key(r: &ObjectRefOwned) -> ObjectKey {
    (
        r.name_as_str().unwrap_or_default().to_owned(),
        r.path_as_str().to_owned(),
    )
}

fn is_unknown_error(err: &zbus::Error) -> bool {
    matches!(
        err,
        zbus::Error::MethodError(name, _, _)
            if matches!(
                name.as_str(),
                "org.freedesktop.DBus.Error.UnknownInterface"
                    | "org.freedesktop.DBus.Error.UnknownMethod"
                    | "org.freedesktop.DBus.Error.UnknownObject"
            )
    )
}

fn is_null(item: &ObjectRefOwned) -> bool {
    item.is_null() || item.path() == "/org/a11y/atspi/null"
}

struct LruMap<V> {
    map: HashMap<ObjectKey, (u64, V)>,
    tick: u64,
    limit: usize,
}

impl<V> LruMap<V> {
    fn new(limit: usize) -> Self {
        Self {
            map: HashMap::new(),
            tick: 0,
            limit,
        }
    }

    fn get(&mut self, key: &ObjectKey) -> Option<&V> {
        self.map.get_mut(key).map(|entry| {
            entry.0 = self.tick;
            &entry.1
        })
    }

    fn insert(&mut self, key: ObjectKey, value: V) {
        self.tick += 1;
        if self.map.len() >= self.limit && !self.map.contains_key(&key) {
            let oldest = self
                .map
                .iter()
                .min_by_key(|(_, (t, _))| *t)
                .map(|(k, _)| k.clone());
            if let Some(oldest) = oldest {
                self.map.remove(&oldest);
            }
        }
        self.map.insert(key, (self.tick, value));
    }

    fn retain(&mut self, mut f: impl FnMut(&ObjectKey, &V) -> bool) {
        self.map.retain(|k, (_, v)| f(k, v));
    }
}
