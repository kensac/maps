# Maps

A simple map image generator if I can even call it that.

It is a simple script that parses osm.pbf data and generates a map image. It wll generate roads, waterways, railways, buildings and natural land features. This is not meant to be a full featured map generator but is a simple script I wrote in a weekend to understand how maps are generated.

It also has A\* path finding algorithm implemented to find the shortest path between two points.

## Getting Started

Make sure you have atleast 6x the size of the osm.pbf file in free space. The program will generate a lot of temporary files.

To run the project, clone the repository and run the following commands:

```
$ git clone <repo>
$ cd <repo>
$ cargo build --release
```

Download the osm.pbf file from [bbbike](https://extract.bbbike.org/). You can download the file for any region you want. Make sure it is in the osm.pbf format.

Then run the following command to generate the map image:

```
$ ./target/release/maps <osm.pbf file>
```

To change the size of the image tiling, change the 
`tiles_x` ,`tiles_y` and `img_size` variables in the `src/drawing.rs` file.

### Example





<!--
### Prerequisites
 ### Installing

A step by step series of examples that tell you how to get a development env running

Say what the step will be

```
Give the example
```

And repeat

```
until finished
```

End with an example of getting some data out of the system or using it for a little demo

## Running the tests

Explain how to run the automated tests for this system

### Break down into end to end tests

Explain what these tests test and why

```
Give an example
```

### And coding style tests

Explain what these tests test and why

```
Give an example
```

## Deployment

Add additional notes about how to deploy this on a live system

## Built With

* [Dropwizard](http://www.dropwizard.io/1.0.2/docs/) - The web framework used
* [Maven](https://maven.apache.org/) - Dependency Management
* [ROME](https://rometools.github.io/rome/) - Used to generate RSS Feeds

## Contributing

Please read [CONTRIBUTING.md](https://gist.github.com/PurpleBooth/b24679402957c63ec426) for details on our code of conduct, and the process for submitting pull requests to us.

## Versioning

We use [SemVer](http://semver.org/) for versioning. For the versions available, see the [tags on this repository](https://github.com/your/project/tags).

## Authors

* **Billie Thompson** - *Initial work* - [PurpleBooth](https://github.com/PurpleBooth)

See also the list of [contributors](https://github.com/your/project/contributors) who participated in this project.

## License

This project is licensed under the MIT License - see the [LICENSE.md](LICENSE.md) file for details

## Acknowledgments

 -->
