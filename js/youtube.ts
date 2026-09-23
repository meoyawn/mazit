import {Innertube, Platform, Player, YTNodes, Log, Constants} from 'youtubei.js';

declare function hostFetch(request: string): Promise<string>;
declare function hostCookie(): string;
declare const Buffer: {from(value: string | ArrayBuffer, encoding?: string): {toString(encoding: string): string}};

async function fetchThroughRust(input: RequestInfo | URL, init?: RequestInit) {
  const request = new Request(input, init);
  const body = request.method === 'GET' || request.method === 'HEAD' ? null : Buffer.from(await request.arrayBuffer()).toString('base64');
  const result = JSON.parse(await hostFetch(JSON.stringify({url: request.url, method: request.method, headers: [...request.headers], body})));
  if (result.error) throw new Error(result.error);
  return new Response(Buffer.from(result.body, 'base64') as unknown as BodyInit, {status: result.status, headers: result.headers});
}

Platform.load({
  runtime: 'unknown', server: true,
  fetch: fetchThroughRust, Request, Response, Headers, FormData, File,
  ReadableStream, CustomEvent,
  uuidv4: () => crypto.randomUUID(),
  sha1Hash: async (text: string) => Buffer.from(await crypto.subtle.digest('SHA-1', new TextEncoder().encode(text))).toString('hex'),
  eval: async (data, env) => new Function(...Object.keys(env), data.output)(...Object.values(env)),
} as Parameters<typeof Platform.load>[0]);
Log.setLevel(Log.Level.NONE);

let client: Promise<Innertube> | undefined;
const pages = new Map<string, Awaited<ReturnType<Innertube['getPlaylist']>>>();
const cache = new Map<string, ArrayBuffer>();

function session() {
  client ??= Innertube.create({lang: 'en', location: 'US', retrieve_player: false, cookie: hostCookie() || undefined,
    cache: {cache_dir: '', get: async key => cache.get(key), set: async (key, value) => { cache.set(key, value); }, remove: async key => { cache.delete(key); }},
  }).catch(error => { client = undefined; throw error; });
  return client;
}

export async function call(method: string, json: string): Promise<string> {
  try { return await operation(method, json); }
  catch (reason) {
    const error = reason as Error;
    return JSON.stringify({bridgeError: error?.message || String(reason)});
  }
}

async function operation(method: string, json: string): Promise<string> {
  const args = JSON.parse(json);
  const yt = await session();
  if (method === 'resolve') {
    const endpoint = await yt.resolveURL(args.url);
    return JSON.stringify({id: endpoint.payload.browseId});
  }
  if (method === 'page') {
    const previous = pages.get(args.id);
    const page = args.continuation && previous ? await previous.getContinuation() : await yt.getPlaylist(args.id);
    if (page.has_continuation) pages.set(args.id, page); else pages.delete(args.id);
    const videos = page.items.map(item => {
      if (item.is(YTNodes.PlaylistVideo)) return {id: item.id, title: item.title.toString(), duration: item.duration.seconds || 0, available: item.is_playable && !item.is_live && !item.is_upcoming};
      if (item.is(YTNodes.LockupView) && ['VIDEO', 'SHORT'].includes(item.content_type)) return {id: item.content_id, title: item.metadata?.title.toString() || item.content_id, duration: 0, available: true};
      throw new Error('Unsupported playlist item; listing is incomplete.');
    });
    return JSON.stringify({title: page.info.title, description: page.info.description || '', count: page.info.total_items, continuation: page.has_continuation, suspicious: !!page.messages?.length, videos});
  }
  if (method === 'channel') {
    const channel = await yt.getChannel(args.id);
    return JSON.stringify({title: channel.metadata.title, description: channel.metadata.description || ''});
  }
  if (method === 'media') {
    const info = await yt.getBasicInfo(args.id, {client: args.client});
    if (info.playability_status?.status !== 'OK') throw new Error(`YouTube playback unavailable: ${info.playability_status?.reason || 'unknown reason'}`);
    if (info.basic_info.is_live || info.basic_info.is_upcoming) throw new Error('Video is live or upcoming.');
    const formats = info.streaming_data?.adaptive_formats.filter(item => item.has_audio && !item.has_video && !item.drm_families?.length && !item.is_type_otf && !!(item.url || item.cipher || item.signature_cipher)) || [];
    const aac = formats.filter(item => item.mime_type.includes('audio/mp4') && item.mime_type.includes('mp4a'));
    const selected = (aac.length ? aac : formats).sort((a, b) => b.bitrate - a.bitrate)[0];
    if (!selected) throw new Error('No downloadable audio format.');
    const microformat = info.page[0].microformat;
    const published = microformat?.is(YTNodes.PlayerMicroformat) ? microformat.publish_date || microformat.upload_date : null;
    if (selected.cipher || selected.signature_cipher || (selected.url && new URL(selected.url).searchParams.has('n'))) {
      yt.session.player ??= await Player.create(yt.session.cache, fetchThroughRust);
    }
    const url = new URL(await selected.decipher(yt.session.player));
    url.searchParams.set('cpn', info.cpn);
    const clientConstants = Constants.CLIENTS[args.client as keyof typeof Constants.CLIENTS];
    const userAgent = clientConstants && 'USER_AGENT' in clientConstants ? clientConstants.USER_AGENT : yt.session.context.client.userAgent;
    return JSON.stringify({url: url.toString(), bytes: selected.content_length || 0, user_agent: userAgent,
      title: info.basic_info.title || args.id, description: info.basic_info.short_description || '', duration: info.basic_info.duration || 0, published});
  }
  throw new Error('Unknown YouTube operation.');
}
