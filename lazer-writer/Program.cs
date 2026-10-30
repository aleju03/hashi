// Changes which files one of lazer's skins is made of, in its database, the way lazer's own
// "Edit externally" does when it takes a skin back in. The files are already in lazer's file store
// under their hashes; this points the skin's names at them, all in one write or none of it.
//
// LazerWriter <job.json>, the job { realm, skin, set: [{ name, hash }], remove: [name], result }.
// The result file gets { ok } or { ok: false, error }.

using System.Text.Json;
using System.Text.Json.Nodes;
using Realms;

var job = JsonNode.Parse(File.ReadAllText(args[0]))!;
var resultPath = (string)job["result"]!;
try
{
    Run(job);
    File.WriteAllText(resultPath, JsonSerializer.Serialize(new { ok = true }));
}
catch (Exception e)
{
    File.WriteAllText(resultPath, JsonSerializer.Serialize(new { ok = false, error = e.Message }));
}

static void Run(JsonNode job)
{
    var id = Guid.Parse((string)job["skin"]!);
    var set = job["set"]!.AsArray().Select(s => (name: Standard((string)s!["name"]!), hash: (string)s["hash"]!)).ToList();
    var remove = job["remove"]!.AsArray().Select(n => Standard((string)n!)).ToHashSet(StringComparer.OrdinalIgnoreCase);
    foreach (var (name, hash) in set)
    {
        if (hash.Length != 64 || !hash.All(Uri.IsHexDigit))
            throw new Exception($"{name} has no proper hash.");
        if (name.Length == 0 || name.Split('/').Any(p => p is "" or "." or ".."))
            throw new Exception($"{name} isn't a name lazer allows.");
    }

    // dynamic: the schema is whatever lazer's database has, this knows only the parts it touches
    var config = new RealmConfiguration((string)job["realm"]!) { IsDynamic = true };
    using var realm = Realm.GetInstance(config);
    var skin = realm.DynamicApi.Find("Skin", id) ?? throw new Exception("lazer doesn't have this skin anymore.");
    if (skin.DynamicApi.Get<bool>("Protected"))
        throw new Exception("lazer's own skins can't be changed.");

    realm.Write(() =>
    {
        var files = skin.DynamicApi.GetList<IEmbeddedObject>("Files");
        IEmbeddedObject? Named(string name) => files.FirstOrDefault(f => string.Equals(Standard(f.DynamicApi.Get<string>("Filename")), name, StringComparison.OrdinalIgnoreCase));

        foreach (var name in remove)
            if (Named(name) is { } gone)
                files.Remove(gone);
        foreach (var (name, hash) in set)
        {
            var file = realm.DynamicApi.Find("File", hash) ?? realm.DynamicApi.CreateObject("File", hash);
            var usage = Named(name);
            if (usage == null)
            {
                usage = realm.DynamicApi.AddEmbeddedObjectToList(files);
                usage.DynamicApi.Set("Filename", name);
            }
            usage.DynamicApi.Set("File", RealmValue.Object(file));
        }

        // an exception here undoes the whole write: lazer can't load a skin with a name but no file
        var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var f in files)
        {
            var name = f.DynamicApi.Get<string>("Filename");
            if (f.DynamicApi.Get<IRealmObject?>("File") == null)
                throw new Exception($"{name} came out without a file, nothing was saved.");
            if (!seen.Add(Standard(name)))
                throw new Exception($"{name} came out twice, nothing was saved.");
        }
        foreach (var (name, hash) in set)
            if (Named(name)?.DynamicApi.Get<IRealmObject?>("File")?.DynamicApi.Get<string>("Hash") != hash)
                throw new Exception($"{name} didn't take, nothing was saved.");
    });
}

// lazer keeps names with forward slashes
static string Standard(string name) => name.Replace('\\', '/').Trim('/');
